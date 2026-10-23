#!/usr/bin/env python3
"""Package only public runtime files and emit the checksum-pinned Homebrew formula."""
import argparse
import gzip
import hashlib
import io
from pathlib import Path
import platform
import re
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/amber-shell')
    parser.add_argument('--output', type=Path, default=ROOT / 'work/release')
    args = parser.parse_args()
    if platform.system() != 'Darwin' or platform.machine() != 'arm64':
        parser.error('Release packaging currently supports macOS Apple Silicon only')
    version = re.search(r'^version = "([0-9]+\.[0-9]+\.[0-9]+)"$',
                        (ROOT / 'Cargo.toml').read_text(), re.M).group(1)
    binary = args.binary.resolve(strict=True)
    actual = subprocess.check_output([str(binary), '--version'], text=True).strip()
    if actual != 'amber-shell ' + version:
        parser.error('Binary version differs from Cargo.toml; rebuild the release binary')
    arch = subprocess.check_output(['/usr/bin/lipo', '-archs', str(binary)], text=True).strip()
    if arch != 'arm64':
        parser.error('Release binary must contain exactly the arm64 architecture')
    subprocess.run(['/usr/bin/codesign', '--verify', str(binary)], check=True)
    files = [
        (binary, 'bin/amber-shell', 0o755),
        (ROOT / 'shell/amber-shell.zsh', 'share/amber-shell/amber-shell.zsh', 0o644),
        (ROOT / 'shell/amber-session.zsh', 'share/amber-shell/amber-session.zsh', 0o644),
        (ROOT / 'scripts/install.sh', 'share/amber-shell/install.sh', 0o644),
        (ROOT / 'LICENSE', 'LICENSE', 0o644),
    ]
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / ('amber-shell-' + version + '-aarch64-apple-darwin.tar.gz')
    # Fixed metadata makes packaging identical inputs byte-for-byte reproducible.
    with archive.open('wb') as stream:
        with gzip.GzipFile(fileobj=stream, mode='wb', filename='', mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as tar:
                for source, name, mode in files:
                    content = source.read_bytes()
                    info = tarfile.TarInfo(name)
                    info.size, info.mode = len(content), mode
                    tar.addfile(info, io.BytesIO(content))
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    (args.output / 'SHA256SUMS').write_text(checksum + '  ' + archive.name + '\n')
    formula = (ROOT / 'packaging/homebrew/amber-shell.rb.in').read_text()
    formula = formula.replace('@VERSION@', version).replace('@SHA256@', checksum)
    (args.output / 'amber-shell.rb').write_text(formula)
    print(f'{archive}: {archive.stat().st_size} bytes, sha256 {checksum}')
    print('Generated amber-shell.rb: copy to marcdon2002/homebrew-tap/Formula/ after release.')


if __name__ == '__main__':
    main()
