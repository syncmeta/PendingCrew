#!/bin/sh
# Release renderer binds the immutable installer and its digest below.
set -eu
case "$(uname -s):$(uname -m)" in
  Linux:x86_64) ;;
  *) echo "PendingCrew currently provides a qualified Linux amd64 installer only." >&2; exit 1 ;;
esac
command -v python3 >/dev/null 2>&1 || { echo "Install Python 3.9 or newer first." >&2; exit 1; }
exec python3 -I - "$@" <<'PY'
import hashlib, os, pathlib, shutil, subprocess, sys, tempfile, urllib.request
if '--help' in sys.argv[1:]:
    print("""PendingCrew Linux amd64 installer
Requires Linux x86_64, glibc @MIN_GLIBC@+, Python 3.9+, running systemd.
Run with sudo; interactively choose an existing ordinary account and connection.
Options: --user USER --codex PATH --claude PATH --disable-claude
         --advertise-host HOST --listen-host IP --port PORT
         --non-interactive --accept-loopback-tunnel
Non-interactive mode requires user, advertise-host, listen-host and port.
Providers must already be installed and authenticated by that ordinary user.
After installation: sudo pendingcrew status; sudo pendingcrew pair
Existing installs require explicit safe upgrade/original UUID recovery.""")
    sys.exit(0)
if sys.version_info < (3, 9):
    sys.exit("Python 3.9 or newer is required.")
if os.geteuid() != 0:
    sys.exit("Run this installer with sudo; you will select an existing ordinary service user.")
if not pathlib.Path('/run/systemd/system').is_dir() or not shutil.which('systemctl') or not shutil.which('systemd-run'):
    sys.exit("A running systemd host with systemctl and systemd-run is required.")
try:
    libc = os.confstr('CS_GNU_LIBC_VERSION').split()
    compatible = libc[0] == 'glibc' and tuple(map(int, libc[1].split('.'))) >= tuple(map(int, '@MIN_GLIBC@'.split('.')))
except (AttributeError, OSError, ValueError):
    compatible = False
if not compatible:
    sys.exit("This Linux amd64 package requires glibc @MIN_GLIBC@ or newer; musl/Alpine is unsupported.")
url = @INSTALLER_URL@
digest = @INSTALLER_SHA256@
if not url.startswith('https://') or len(digest) != 64:
    sys.exit("Use the rendered release install.sh; this source template is not an installation entry.")
with tempfile.TemporaryDirectory(prefix='pendingcrew-bootstrap-') as directory:
    with urllib.request.urlopen(url, timeout=60) as response:
        if not response.geturl().startswith('https://'):
            sys.exit("Installer redirect must retain HTTPS.")
        payload = response.read(1024 * 1024 + 1)
    if len(payload) > 1024 * 1024 or hashlib.sha256(payload).hexdigest() != digest:
        sys.exit("Immutable installer digest mismatch; nothing was executed.")
    entry = pathlib.Path(directory) / 'install.py'
    entry.write_bytes(payload)
    result = subprocess.run([sys.executable, '-I', str(entry), 'setup', *sys.argv[1:]])
    sys.exit(result.returncode)
PY
