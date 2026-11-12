//! Desktop audio: PulseAudio monitor capture (PulseAudio's native protocol, spoken by
//! PulseAudio and PipeWire, in Rust: no libpulse) -> Opus (10 ms frames) -> clients.

use crate::hub::{Hub, Out};
use bytes::Bytes;
use futures_util::io::AsyncReadExt;
use pulseaudio::protocol;
use std::ffi::{c_int, c_void, CString};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

// Opus is linked statically by audiopus_sys.
extern "C" {
    fn opus_encoder_create(
        fs: i32,
        channels: c_int,
        application: c_int,
        error: *mut c_int,
    ) -> *mut c_void;
    fn opus_encoder_ctl(st: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_encode(
        st: *mut c_void,
        pcm: *const i16,
        frame_size: c_int,
        data: *mut u8,
        max: i32,
    ) -> i32;
}

struct OpusEnc(*mut c_void);
unsafe impl Send for OpusEnc {}

const RATE: u32 = 48000;
const FRAME: usize = 480;

pub fn spawn(device: String, bitrate: i32, hub: Arc<Hub>) {
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            let mut err = 0;
            let enc = unsafe { opus_encoder_create(RATE as i32, 2, 2051, &mut err) };
            if enc.is_null() {
                log::error!("opus init failed: {err}");
                return;
            }
            unsafe { opus_encoder_ctl(enc, 4002, bitrate as c_int) };
            let enc = OpusEnc(enc);
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("audio runtime");
            rt.block_on(run(device, enc, hub));
        })
        .unwrap();
}

fn wanted(hub: &Hub) -> bool {
    hub.clients
        .lock()
        .iter()
        .any(|c| c.audio.load(Ordering::Relaxed))
}

async fn run(device: String, enc: OpusEnc, hub: Arc<Hub>) {
    let dev = CString::new(device).unwrap_or_else(|_| c"@DEFAULT_MONITOR@".to_owned());
    loop {
        if !wanted(&hub) {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        if let Err(e) = capture(&dev, &enc, &hub).await {
            log::warn!("pulse capture: {e}; retrying");
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

/// Records from `dev` until nobody wants audio (Ok) or the server goes away (Err).
async fn capture(dev: &CString, enc: &OpusEnc, hub: &Hub) -> Result<(), String> {
    let client = pulseaudio::Client::from_env(c"voxl").map_err(|e| e.to_string())?;
    let frag = (FRAME * 4) as u32;
    let params = protocol::RecordStreamParams {
        source_name: Some(dev.clone()),
        sample_spec: protocol::SampleSpec {
            format: protocol::SampleFormat::S16Le,
            channels: 2,
            sample_rate: RATE,
        },
        channel_map: protocol::ChannelMap::stereo(),
        buffer_attr: protocol::stream::BufferAttr {
            max_length: frag * 8,
            fragment_size: frag,
            ..Default::default()
        },
        flags: protocol::stream::StreamFlags {
            adjust_latency: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut buffer = pulseaudio::RecordBuffer::new(frag as usize * 16);
    let stream = client
        .create_record_stream(params, buffer.as_record_sink())
        .await
        .map_err(|e| e.to_string())?;
    log::info!("audio capture started");
    let mut raw = vec![0u8; FRAME * 4];
    let mut pcm = vec![0i16; FRAME * 2];
    let mut out = vec![0u8; 4000];
    let mut checked = 0u32;
    let res = loop {
        if let Err(e) = buffer.read_exact(&mut raw).await {
            break Err(e.to_string());
        }
        checked += 1;
        if checked % 50 == 0 && !wanted(hub) {
            break Ok(());
        }
        for (d, s) in pcm.iter_mut().zip(raw.chunks_exact(2)) {
            *d = i16::from_le_bytes([s[0], s[1]]);
        }
        if pcm.iter().all(|&s| s == 0) {
            continue;
        }
        let n = unsafe {
            opus_encode(
                enc.0,
                pcm.as_ptr(),
                FRAME as c_int,
                out.as_mut_ptr(),
                out.len() as i32,
            )
        };
        if n <= 0 {
            continue;
        }
        let mut m = Vec::with_capacity(n as usize + 1);
        m.push(0x02);
        m.extend_from_slice(&out[..n as usize]);
        let b = Bytes::from(m);
        for c in hub.clients.lock().iter() {
            if c.audio.load(Ordering::Relaxed) {
                c.send(Out::Bin(b.clone()));
            }
        }
    };
    let _ = stream.delete().await;
    log::info!("audio capture stopped");
    res
}
