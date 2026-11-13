import type { Shell } from '@/api/types';

/** What a new script starts with, per shell. */
export const scriptTemplate: Record<Shell, string> = {
	powershell: 'Get-ComputerInfo | Select-Object OsName, OsVersion\n',
	cmd: 'ver\n',
	bash: '#!/usr/bin/env bash\nset -euo pipefail\nuname -a\n',
	sh: 'uname -a\n',
};
