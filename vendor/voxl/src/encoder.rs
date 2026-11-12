//! Video encoders for low-latency desktop streaming: VP9 (libvpx, real-time screen
//! content) by default, H.264 (OpenH264) for browsers that can't decode VP9. Both are
//! built from source and linked statically.
//!
//! `crf` keeps voxl's x264 meaning (lower is sharper, on H.264's 0-51 QP scale) and
//! sets each encoder's best allowed quantizer; `max_kbps` is the CBR target that the
//! adaptive bitrate in video.rs moves.

use std::ffi::c_int;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Codec {
    #[default]
    Vp9,
    H264,
}

impl Codec {
    pub fn parse(s: &str) -> Option<Codec> {
        match s.to_ascii_lowercase().as_str() {
            "vp9" | "vp09" => Some(Codec::Vp9),
            "h264" | "avc" | "avc1" => Some(Codec::H264),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Codec::Vp9 => "vp9",
            Codec::H264 => "h264",
        }
    }
}

#[derive(Clone)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub threads: u32,
    pub crf: f32,
    /// CBR target in kbit/s (0 = the encoder's default).
    pub max_kbps: u32,
    pub codec: Codec,
}

pub struct Planes<'a> {
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub stride_y: usize,
    pub stride_uv: usize,
}

/// What the encoder made of one picture.
pub struct Encoded {
    pub keyframe: bool,
    /// The frame's QP on H.264's scale, for video.rs's x264-tuned motion quality
    /// correction. These encoders don't report it the same way, so it's None and that
    /// correction stays off.
    pub qp: Option<f32>,
    pub crf_avg: f32,
}

pub enum Encoder {
    Vp9(vp9::Vp9),
    H264(h264::H264),
}

impl Encoder {
    pub fn new(cfg: &EncoderConfig) -> Result<Self, String> {
        Ok(match cfg.codec {
            Codec::Vp9 => Encoder::Vp9(vp9::Vp9::new(cfg)?),
            Codec::H264 => Encoder::H264(h264::H264::new(cfg)?),
        })
    }

    /// Reconfigure rate control on the fly (buffer of ~1.25 frames at `max_kbps`).
    pub fn set_rate(&mut self, crf: f32, max_kbps: u32) {
        let fps = match self {
            Encoder::Vp9(e) => e.fps,
            Encoder::H264(e) => e.fps,
        };
        self.set_rate_buf(crf, max_kbps, vbv_buffer_kbit(max_kbps, fps));
    }

    /// Reconfigure rate control with an explicit buffer (kbit).
    pub fn set_rate_buf(&mut self, crf: f32, max_kbps: u32, buf_kbit: u32) {
        match self {
            Encoder::Vp9(e) => e.set_rate(crf, max_kbps, buf_kbit),
            Encoder::H264(e) => e.set_rate(max_kbps),
        }
    }

    /// Encode one picture, appending the compressed frame to `out` (Annex B for H.264,
    /// a VP9 frame for VP9). `force_idr` asks for a keyframe. `mb_info` (one byte per
    /// 16x16 block, 1 = unchanged) lets VP9 skip blocks that didn't change.
    pub fn encode(
        &mut self,
        p: &Planes,
        ts_ms: i64,
        force_idr: bool,
        _qp: Option<i32>,
        mb_info: Option<&mut [u8]>,
        out: &mut Vec<u8>,
    ) -> Result<Encoded, String> {
        match self {
            Encoder::Vp9(e) => e.encode(p, ts_ms, force_idr, mb_info, out),
            Encoder::H264(e) => e.encode(p, ts_ms, force_idr, out),
        }
    }
}

/// A buffer of 1.25 frames at `max_kbps`, rounded up.
fn vbv_buffer_kbit(max_kbps: u32, fps: u32) -> u32 {
    (max_kbps * 5).div_ceil(4 * fps.max(1))
}

/// H.264's 0-51 QP scale onto VP9's 0-63 quantizer scale.
fn vp9_q(crf: f32) -> u32 {
    (crf * 63.0 / 51.0).round().clamp(0.0, 63.0) as u32
}

mod vp9 {
    use super::*;

    #[allow(
        non_camel_case_types,
        non_upper_case_globals,
        non_snake_case,
        dead_code,
        unused_imports,
        clippy::all
    )]
    mod ffi {
        include!(concat!(env!("OUT_DIR"), "/vpx.rs"));
    }

    pub struct Vp9 {
        ctx: ffi::vpx_codec_ctx_t,
        cfg: ffi::vpx_codec_enc_cfg_t,
        img: ffi::vpx_image_t,
        pts: i64,
        pub fps: u32,
        mb_cols: u32,
        mb_rows: u32,
        active: Vec<u8>,
    }

    unsafe impl Send for Vp9 {}

    fn check(r: ffi::vpx_codec_err_t, what: &str) -> Result<(), String> {
        if r == ffi::vpx_codec_err_t_VPX_CODEC_OK {
            Ok(())
        } else {
            let msg = unsafe { std::ffi::CStr::from_ptr(ffi::vpx_codec_err_to_string(r)) };
            Err(format!("{what}: {}", msg.to_string_lossy()))
        }
    }

    impl Vp9 {
        pub fn new(c: &EncoderConfig) -> Result<Self, String> {
            unsafe {
                let iface = ffi::vpx_codec_vp9_cx();
                let mut cfg: ffi::vpx_codec_enc_cfg_t = std::mem::zeroed();
                check(
                    ffi::vpx_codec_enc_config_default(iface, &mut cfg, 0),
                    "vpx config",
                )?;
                cfg.g_w = c.width;
                cfg.g_h = c.height;
                cfg.g_timebase = ffi::vpx_rational { num: 1, den: 1000 };
                cfg.g_threads = c.threads.max(1);
                cfg.g_lag_in_frames = 0;
                cfg.g_error_resilient = ffi::VPX_ERROR_RESILIENT_DEFAULT;
                cfg.g_pass = ffi::vpx_enc_pass_VPX_RC_ONE_PASS;
                cfg.rc_end_usage = ffi::vpx_rc_mode_VPX_CBR;
                cfg.rc_dropframe_thresh = 0;
                cfg.rc_resize_allowed = 0;
                cfg.rc_undershoot_pct = 50;
                cfg.rc_overshoot_pct = 50;
                // Keyframes only when asked for (a new viewer or a lost frame).
                cfg.kf_mode = ffi::vpx_kf_mode_VPX_KF_DISABLED;
                let mut e = Vp9 {
                    ctx: std::mem::zeroed(),
                    cfg,
                    img: std::mem::zeroed(),
                    pts: 0,
                    fps: c.fps.max(1),
                    mb_cols: c.width.div_ceil(16),
                    mb_rows: c.height.div_ceil(16),
                    active: Vec::new(),
                };
                e.apply_rate(c.crf, c.max_kbps, vbv_buffer_kbit(c.max_kbps, c.fps));
                check(
                    ffi::vpx_codec_enc_init_ver(
                        &mut e.ctx,
                        iface,
                        &e.cfg,
                        0,
                        ffi::VPX_ENCODER_ABI_VERSION as c_int,
                    ),
                    "vpx init",
                )?;
                let ctl = |e: &mut Vp9, id: u32, v: c_int| {
                    ffi::vpx_codec_control_(&mut e.ctx, id as c_int, v);
                };
                ctl(&mut e, ffi::vp8e_enc_control_id_VP8E_SET_CPUUSED, 8);
                ctl(
                    &mut e,
                    ffi::vp8e_enc_control_id_VP9E_SET_TUNE_CONTENT,
                    ffi::vp9e_tune_content_VP9E_CONTENT_SCREEN as c_int,
                );
                // Cyclic refresh: the RTC adaptive quantizer.
                ctl(&mut e, ffi::vp8e_enc_control_id_VP9E_SET_AQ_MODE, 3);
                ctl(&mut e, ffi::vp8e_enc_control_id_VP9E_SET_ROW_MT, 1);
                let tiles = (c.threads.max(1) as f32).log2().floor() as c_int;
                ctl(&mut e, ffi::vp8e_enc_control_id_VP9E_SET_TILE_COLUMNS, tiles);
                ctl(&mut e, ffi::vp8e_enc_control_id_VP9E_SET_NOISE_SENSITIVITY, 0);
                ctl(&mut e, ffi::vp8e_enc_control_id_VP8E_SET_STATIC_THRESHOLD, 0);
                // Keyframes may take several frames' worth: they're what a viewer sees first.
                ctl(&mut e, ffi::vp8e_enc_control_id_VP8E_SET_MAX_INTRA_BITRATE_PCT, 900);
                // sRGB content in BT.601 limited range, as convert.rs produces it.
                ctl(
                    &mut e,
                    ffi::vp8e_enc_control_id_VP9E_SET_COLOR_SPACE,
                    ffi::vpx_color_space_VPX_CS_BT_601 as c_int,
                );
                ctl(
                    &mut e,
                    ffi::vp8e_enc_control_id_VP9E_SET_COLOR_RANGE,
                    ffi::vpx_color_range_VPX_CR_STUDIO_RANGE as c_int,
                );
                if ffi::vpx_img_wrap(
                    &mut e.img,
                    ffi::vpx_img_fmt_VPX_IMG_FMT_I420,
                    c.width,
                    c.height,
                    1,
                    std::ptr::NonNull::<u8>::dangling().as_ptr(),
                )
                .is_null()
                {
                    ffi::vpx_codec_destroy(&mut e.ctx);
                    return Err("vpx_img_wrap failed".into());
                }
                Ok(e)
            }
        }

        fn apply_rate(&mut self, crf: f32, max_kbps: u32, buf_kbit: u32) {
            let c = &mut self.cfg;
            if max_kbps > 0 {
                c.rc_target_bitrate = max_kbps;
                // The buffer in ms of the target rate; libvpx's CBR wants some room.
                let ms = (buf_kbit as u64 * 1000 / max_kbps as u64).clamp(40, 1000) as u32;
                c.rc_buf_sz = ms;
                c.rc_buf_initial_sz = ms * 4 / 5;
                c.rc_buf_optimal_sz = ms * 4 / 5;
            }
            c.rc_min_quantizer = vp9_q(crf).min(56);
            c.rc_max_quantizer = 63;
        }

        pub fn set_rate(&mut self, crf: f32, max_kbps: u32, buf_kbit: u32) {
            self.apply_rate(crf, max_kbps, buf_kbit);
            unsafe {
                if let Err(e) = check(
                    ffi::vpx_codec_enc_config_set(&mut self.ctx, &self.cfg),
                    "vpx reconfigure",
                ) {
                    log::warn!("{e}");
                }
            }
        }

        pub fn encode(
            &mut self,
            p: &Planes,
            ts_ms: i64,
            force_idr: bool,
            mb_info: Option<&mut [u8]>,
            out: &mut Vec<u8>,
        ) -> Result<Encoded, String> {
            unsafe {
                self.img.planes[0] = p.y.as_ptr() as *mut u8;
                self.img.planes[1] = p.u.as_ptr() as *mut u8;
                self.img.planes[2] = p.v.as_ptr() as *mut u8;
                self.img.stride[0] = p.stride_y as c_int;
                self.img.stride[1] = p.stride_uv as c_int;
                self.img.stride[2] = p.stride_uv as c_int;
                // Blocks that didn't change are skipped (the active map is in 16x16
                // blocks, like the damage map); keyframes code everything.
                let n = (self.mb_cols * self.mb_rows) as usize;
                let mut map = ffi::vpx_active_map_t {
                    active_map: std::ptr::null_mut(),
                    rows: self.mb_rows,
                    cols: self.mb_cols,
                };
                match mb_info {
                    Some(info) if !force_idr && info.len() >= n => {
                        self.active.clear();
                        self.active.extend(info[..n].iter().map(|&u| (u == 0) as u8));
                        map.active_map = self.active.as_mut_ptr();
                    }
                    _ => {}
                }
                ffi::vpx_codec_control_(
                    &mut self.ctx,
                    ffi::vp8e_enc_control_id_VP8E_SET_ACTIVEMAP as c_int,
                    &mut map as *mut ffi::vpx_active_map_t,
                );
                self.pts = ts_ms.max(self.pts + 1);
                let flags = if force_idr {
                    ffi::VPX_EFLAG_FORCE_KF as ffi::vpx_enc_frame_flags_t
                } else {
                    0
                };
                check(
                    ffi::vpx_codec_encode(
                        &mut self.ctx,
                        &self.img,
                        self.pts,
                        (1000 / self.fps) as _,
                        flags,
                        ffi::VPX_DL_REALTIME as _,
                    ),
                    "vpx encode",
                )?;
                let mut keyframe = false;
                let mut iter: ffi::vpx_codec_iter_t = std::ptr::null();
                loop {
                    let pkt = ffi::vpx_codec_get_cx_data(&mut self.ctx, &mut iter);
                    if pkt.is_null() {
                        break;
                    }
                    if (*pkt).kind == ffi::vpx_codec_cx_pkt_kind_VPX_CODEC_CX_FRAME_PKT {
                        let f = &(*pkt).data.frame;
                        out.extend_from_slice(std::slice::from_raw_parts(
                            f.buf as *const u8,
                            f.sz,
                        ));
                        keyframe |= f.flags & ffi::VPX_FRAME_IS_KEY != 0;
                    }
                }
                Ok(Encoded {
                    keyframe,
                    qp: None,
                    crf_avg: 0.0,
                })
            }
        }
    }

    impl Drop for Vp9 {
        fn drop(&mut self) {
            unsafe {
                ffi::vpx_codec_destroy(&mut self.ctx);
            }
        }
    }
}

mod h264 {
    use super::*;
    use openh264::encoder::{
        BitRate, Encoder, EncoderConfig as OhConfig, FrameRate, FrameType, IntraFramePeriod,
        QpRange, RateControlMode, UsageType,
    };
    use openh264::formats::YUVSource;
    use openh264::OpenH264API;
    use openh264_sys2::{
        SBitrateInfo, ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE, SPATIAL_LAYER_ALL,
    };

    pub struct H264 {
        enc: Encoder,
        width: usize,
        height: usize,
        pub fps: u32,
        kbps: u32,
    }

    struct Src<'a> {
        p: &'a Planes<'a>,
        w: usize,
        h: usize,
    }

    impl YUVSource for Src<'_> {
        fn dimensions(&self) -> (usize, usize) {
            (self.w, self.h)
        }
        fn strides(&self) -> (usize, usize, usize) {
            (self.p.stride_y, self.p.stride_uv, self.p.stride_uv)
        }
        fn y(&self) -> &[u8] {
            self.p.y
        }
        fn u(&self) -> &[u8] {
            self.p.u
        }
        fn v(&self) -> &[u8] {
            self.p.v
        }
    }

    impl H264 {
        pub fn new(c: &EncoderConfig) -> Result<Self, String> {
            let kbps = if c.max_kbps > 0 { c.max_kbps } else { 8000 };
            let min_qp = (c.crf.round() as i32).clamp(10, 45) as u8;
            let cfg = OhConfig::new()
                .usage_type(UsageType::ScreenContentRealTime)
                .rate_control_mode(RateControlMode::Bitrate)
                .bitrate(BitRate::from_bps(kbps * 1000))
                .max_frame_rate(FrameRate::from_hz(c.fps as f32))
                .skip_frames(false)
                .intra_frame_period(IntraFramePeriod::from_num_frames(0))
                .qp(QpRange::new(min_qp, 51))
                .num_threads(c.threads.clamp(1, 4) as u16);
            let enc = Encoder::with_api_config(OpenH264API::from_source(), cfg)
                .map_err(|e| format!("openh264 init: {e}"))?;
            Ok(H264 {
                enc,
                width: c.width as usize,
                height: c.height as usize,
                fps: c.fps.max(1),
                kbps,
            })
        }

        pub fn set_rate(&mut self, max_kbps: u32) {
            if max_kbps == 0 || max_kbps == self.kbps {
                return;
            }
            self.kbps = max_kbps;
            for opt in [ENCODER_OPTION_BITRATE, ENCODER_OPTION_MAX_BITRATE] {
                let mut info = SBitrateInfo {
                    iLayer: SPATIAL_LAYER_ALL,
                    iBitrate: (max_kbps * 1000) as c_int,
                };
                unsafe {
                    self.enc
                        .raw_api()
                        .set_option(opt, (&raw mut info).cast());
                }
            }
        }

        pub fn encode(
            &mut self,
            p: &Planes,
            _ts_ms: i64,
            force_idr: bool,
            out: &mut Vec<u8>,
        ) -> Result<Encoded, String> {
            if force_idr {
                self.enc.force_intra_frame();
            }
            let src = Src {
                p,
                w: self.width,
                h: self.height,
            };
            let bs = self
                .enc
                .encode(&src)
                .map_err(|e| format!("openh264 encode: {e}"))?;
            let ft = bs.frame_type();
            if !matches!(ft, FrameType::Skip | FrameType::Invalid) {
                bs.write_vec(out);
            }
            Ok(Encoded {
                keyframe: matches!(ft, FrameType::IDR),
                qp: None,
                crf_avg: 0.0,
            })
        }
    }
}
