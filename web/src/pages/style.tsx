import * as React from 'react';
import { Play, Power, Trash2 } from 'lucide-react';
import { useCan } from '@/api/hooks';
import { AttentionBadges, barColor } from '@/components/devices';
import { Button } from '@/components/ui/button';
import { Badge, Chip, Empty, Gauge, KV, Meter, PageHeader, Panel, StatusBadge } from '@/components/ui/display';
import { Checkbox, Field, Input, Select } from '@/components/ui/form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/overlay';
import { Table, Td, Th, Tr } from '@/components/ui/table';

const TOKENS: [string, string][] = [
	['--dh-navy-950', 'Navy 950'],
	['--dh-navy-900', 'Navy 900 · sidebar'],
	['--dh-navy-800', 'Navy 800'],
	['--dh-navy-700', 'Navy 700 · active'],
	['--dh-navy-600', 'Navy 600'],
	['--dh-signal', 'Signal · primary, alerts'],
	['--dh-ok', 'OK · online'],
	['--dh-off', 'Off · offline'],
	['--dh-warn', 'Warn · attention'],
	['--dh-bad', 'Bad · failed'],
	['--dh-run', 'Run · in progress'],
	['--dh-bg', 'Background'],
	['--dh-surface', 'Surface'],
	['--dh-surface-2', 'Surface 2'],
	['--dh-line', 'Line'],
	['--dh-line-strong', 'Line strong'],
	['--dh-fg', 'Text'],
	['--dh-muted', 'Muted'],
	['--dh-subtle', 'Subtle'],
];

function Swatch({ v, label }: { v: string; label: string }) {
	const [value, setValue] = React.useState('');
	React.useEffect(() => {
		setValue(getComputedStyle(document.documentElement).getPropertyValue(v).trim());
	}, [v]);
	return (
		<div className="flex items-center gap-2 rounded border border-line bg-surface p-2">
			<span className="size-8 shrink-0 rounded-sm border border-line" style={{ background: `var(${v})` }} />
			<div className="min-w-0">
				<div className="truncate text-[13px]">{label}</div>
				<div className="truncate font-mono text-[11px] text-muted">
					{v} {value}
				</div>
			</div>
		</div>
	);
}

/** Admin-only reference of the theme's tokens and components, for tuning the look. */
export function StylePage() {
	const ok = useCan('admin');
	const [checked, setChecked] = React.useState(true);
	if (!ok) return <Empty>Only admins can see this page.</Empty>;
	const sample = [
		{ name: 'FRONTDESK-01', os: 'Windows 11 Pro 24H2', ip: '10.0.4.21', online: true, attention: [] as never[] },
		{ name: 'build-runner', os: 'Ubuntu 26.04', ip: '10.0.9.3', online: true, attention: ['reboot_pending', 'low_disk'] as const },
		{ name: 'warehouse-pos', os: 'Windows 10 Pro', ip: '10.0.7.40', online: false, attention: ['offline_7d'] as const },
	];
	return (
		<>
			<PageHeader title="Style guide" sub="Tokens live in web/src/theme.css; change them there to restyle everything." />
			<div className="grid gap-4">
				<Panel title="Color tokens">
					<div className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-4">
						{TOKENS.map(([v, l]) => (
							<Swatch key={v} v={v} label={l} />
						))}
					</div>
				</Panel>
				<Panel title="Type">
					<div className="grid gap-2">
						<div className="text-xl font-semibold">Page title · IBM Plex Sans 600</div>
						<div className="text-[13px]">Body · IBM Plex Sans 400, 13–14px</div>
						<div className="label-caps">Section label · caps 11px</div>
						<div className="font-mono text-[13px]">JetBrains Mono · 10.0.4.21 · 5CD1234XYZ · v1.17.0</div>
						<div className="font-mono text-3xl tabular">1,234</div>
					</div>
				</Panel>
				<Panel title="Instrument tiles">
					<div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
						<Gauge label="Online" value={42} tone="ok" sub="of 50" />
						<Gauge label="Offline" value={8} tone="off" />
						<Gauge label="Needs attention" value={5} tone="warn" />
						<Gauge label="Failed runs" value={2} tone="bad" sub="1 running" />
					</div>
				</Panel>
				<Panel title="Buttons and controls">
					<div className="flex flex-wrap items-center gap-2">
						<Button variant="primary">
							<Play /> Primary
						</Button>
						<Button>Secondary</Button>
						<Button variant="navy">Navy</Button>
						<Button variant="ghost">Ghost</Button>
						<Button variant="danger">
							<Trash2 /> Danger
						</Button>
						<Button size="sm">Small</Button>
						<Button size="icon" aria-label="Power">
							<Power />
						</Button>
						<Button disabled>Disabled</Button>
					</div>
					<div className="mt-3 grid max-w-2xl gap-3 sm:grid-cols-3">
						<Field label="Input">
							<Input placeholder="Placeholder" />
						</Field>
						<Field label="Select">
							<Select>
								<option>Option</option>
							</Select>
						</Field>
						<Field label="Checkbox">
							<span className="flex h-control items-center gap-2">
								<Checkbox checked={checked} onCheckedChange={setChecked} aria-label="Sample" /> Selected
							</span>
						</Field>
					</div>
				</Panel>
				<Panel title="Badges and chips">
					<div className="flex flex-wrap items-center gap-2">
						<StatusBadge status="ok" />
						<StatusBadge status="running" />
						<StatusBadge status="queued" />
						<StatusBadge status="failed" />
						<StatusBadge status="offline" />
						<Badge tone="signal">Signal</Badge>
						<Chip>Windows 11 Pro</Chip>
						<Chip>192.168.1.20</Chip>
						<Chip>5CD1234XYZ</Chip>
					</div>
				</Panel>
				<Panel title="Device rows" bodyClassName="p-0">
					<Table>
						<thead>
							<tr>
								<Th>Name</Th>
								<Th>Status</Th>
								<Th>OS</Th>
								<Th>IP</Th>
								<Th>Attention</Th>
							</tr>
						</thead>
						<tbody>
							{sample.map((d) => (
								<Tr key={d.name} className="status-bar" style={{ ['--bar' as string]: barColor({ online: d.online, attention: [...d.attention] }) }}>
									<Td className="font-medium">{d.name}</Td>
									<Td className="text-xs">{d.online ? <span className="text-ok">Online</span> : <span className="text-off">Seen 9d ago</span>}</Td>
									<Td>
										<Chip>{d.os}</Chip>
									</Td>
									<Td>
										<Chip>{d.ip}</Chip>
									</Td>
									<Td>
										<AttentionBadges d={{ attention: [...d.attention] }} />
									</Td>
								</Tr>
							))}
						</tbody>
					</Table>
				</Panel>
				<div className="grid gap-4 lg:grid-cols-2">
					<Panel title="Key / value">
						<KV rows={[['Hostname', <Chip key="h">build-runner</Chip>], ['Memory', '10.6 GiB of 30.6 GiB'], ['Domain', null]]} />
						<div className="mt-3">
							<Meter used={80} total={100} />
						</div>
					</Panel>
					<Panel title="Tabs">
						<Tabs defaultValue="a">
							<TabsList>
								<TabsTrigger value="a">Overview</TabsTrigger>
								<TabsTrigger value="b">Hardware</TabsTrigger>
							</TabsList>
							<TabsContent value="a" className="pt-3 text-[13px] text-muted">
								Tab content.
							</TabsContent>
							<TabsContent value="b" className="pt-3 text-[13px] text-muted">
								More content.
							</TabsContent>
						</Tabs>
					</Panel>
				</div>
				<Panel title="Output">
					<pre className="rounded bg-navy-950 px-3 py-2 font-mono text-xs text-[#d7e0ec]">
						host=cwwk{'\n'}uid=0{'\n'}
						<span className="text-[#ff8f86]">warning: something on stderr</span>
					</pre>
				</Panel>
			</div>
		</>
	);
}
