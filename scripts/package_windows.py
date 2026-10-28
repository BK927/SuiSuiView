"""Package the tested default x64 build with notices and vendored source."""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = 'x86_64-pc-windows-msvc'
# Windows inbox DLLs only. New imports require an explicit packaging review.
SYSTEM_DLLS = set('''advapi32 bcrypt bcryptprimitives cfgmgr32 comctl32 comdlg32 crypt32 d3d11
d3d12 d3dcompiler_47 dbghelp dwmapi dxgi gdi32 imm32 kernel32 msimg32 msvcrt
mscms ntdll ole32 oleaut32 opengl32 pdh propsys psapi rpcrt4 secur32 setupapi shell32 shlwapi
user32 userenv usp10 uxtheme version winmm winspool wintrust ws2_32 wtsapi32
powrprof hid normaliz shcore runtimeobject windowscodecs'''.split())


def run(*args: str, cwd: Path = ROOT) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True, encoding='utf-8')


def sha256(path: Path) -> str:
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def verify_receipt(receipt: dict, commit: str, root: Path) -> None:
    if receipt.get('commit') != commit or receipt.get('regression_tests') != 'passed':
        raise RuntimeError('No successful build/test receipt for this source commit')
    required = {f'target/{TARGET}/release/suisuiview.exe',
                f'target/{TARGET}/release/suisuiview-cli.exe',
                'target/release-validation/cargo-test.txt',
                'target/release-validation/licenses.json',
                'target/release-validation/decoder-smoke/summary.json'}
    if set(receipt.get('files', {})) != required:
        raise RuntimeError('Incomplete build/test receipt')
    for filename, expected in receipt['files'].items():
        if sha256(root / filename) != expected:
            raise RuntimeError(f'Artifact changed since validation: {filename}')


def check_imports(output: str) -> list[str]:
    dlls = sorted(set(re.findall(r'^\s+([\w.-]+\.dll)\s*$', output, re.MULTILINE | re.IGNORECASE)))
    if not dlls:
        raise RuntimeError('dumpbin did not report any DLL imports')
    unexpected = [d for d in dlls if d.lower()[:-4] not in SYSTEM_DLLS
                  and not d.lower().startswith(('api-ms-win-', 'ext-ms-win-'))]
    if unexpected:
        raise RuntimeError(f'Unreviewed runtime DLL dependencies: {unexpected}')
    return dlls


def dumpbin() -> Path:
    vswhere = Path(os.environ['ProgramFiles(x86)']) / 'Microsoft Visual Studio/Installer/vswhere.exe'
    install = run(str(vswhere), '-latest', '-products', '*', '-requires',
                  'Microsoft.VisualStudio.Component.VC.Tools.x86.x64',
                  '-property', 'installationPath').strip()
    candidates = sorted((Path(install) / 'VC/Tools/MSVC').glob('*/bin/Hostx64/x64/dumpbin.exe'))
    if not candidates:
        raise RuntimeError('VS x64 C++ tools / dumpbin.exe not found')
    return candidates[-1]


def license_report(data: dict) -> tuple[str, set[tuple[str, str]]]:
    parts = ['SuiSuiView Windows x64 dependency licenses\n',
             'Generated from the locked default-feature dependency graph by cargo-about.\n']
    covered = set()
    for license in data['licenses']:
        parts.append(f"\n{license['name']} ({license['id']})\n")
        for usage in license['used_by']:
            crate = usage['crate']
            covered.add((crate['name'], crate['version']))
            parts.append(f"  {crate['name']} {crate['version']} — {crate.get('repository') or 'https://crates.io/crates/' + crate['name']}\n")
        if not license['text'].strip():
            raise RuntimeError('Empty generated license text')
        parts.append('\n' + license['text'] + '\n')
    if not covered:
        raise RuntimeError('Empty dependency license report')
    return ''.join(parts), covered


def main() -> None:
    if os.name != 'nt':
        raise RuntimeError('Package on Windows after running build-windows.ps1')
    if run('git', 'status', '--porcelain', '--untracked-files=normal').strip():
        raise RuntimeError('Commit the public source before packaging; source must match HEAD')
    if os.environ.get('RUSTFLAGS') != '-C target-feature=+crt-static':
        raise RuntimeError('Run scripts/build-windows.ps1 to build the reviewed static-CRT configuration')
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    commit = run('git', 'rev-parse', 'HEAD').strip()
    name = f'SuiSuiView-{version}-windows-x64'
    source_name = f'SuiSuiView-{version}-source'
    validation = ROOT / 'target/release-validation'
    receipt = json.loads((validation / 'build-receipt.json').read_text(encoding='utf-8-sig'))
    verify_receipt(receipt, commit, ROOT)
    licenses, covered = license_report(json.loads((validation / 'licenses.json').read_text(encoding='utf-8')))
    smoke = json.loads((validation / 'decoder-smoke/summary.json').read_text(encoding='utf-8'))
    if smoke.get('required_smoke_passed') is not True or len(smoke.get('cases', [])) != 8:
        raise RuntimeError('Decoder smoke evidence is incomplete')
    compatibility = smoke.get('compatibility_probes', [])
    if len(compatibility) != 2:
        raise RuntimeError('LZMA compatibility probe evidence is missing')
    exe = ROOT / 'target' / TARGET / 'release/suisuiview.exe'
    imports = check_imports(run(str(dumpbin()), '/DEPENDENTS', str(exe)))
    (validation / 'imports.json').write_text(json.dumps(imports, indent=2), encoding='utf-8')
    dist = ROOT / 'dist'
    dist.mkdir(exist_ok=True)
    # TemporaryDirectory owns only a freshly created directory under target.
    with tempfile.TemporaryDirectory(prefix='package-', dir=ROOT / 'target') as temporary:
        stage = Path(temporary)
        bundle, source = stage / name, stage / source_name
        bundle.mkdir()
        source.mkdir()
        archive = stage / 'tracked.zip'
        run('git', 'archive', '--format=zip', '-o', str(archive), 'HEAD')
        with zipfile.ZipFile(archive) as z:
            z.extractall(source)
        # Include ALL locked sources, including the pinned patched winit and
        # native vendor directories. GitHub's automatic source ZIP omits these.
        vendor_config = run('cargo', 'vendor', '--locked', '--versioned-dirs', 'vendor', cwd=source)
        (source / '.cargo').mkdir(exist_ok=True)
        (source / '.cargo/config.toml').write_text(vendor_config, encoding='utf-8')
        metadata = json.loads(run('cargo', 'metadata', '--locked', '--offline',
                                  '--format-version', '1', '--filter-platform', TARGET, cwd=source))
        nodes = {n['id']: n for n in metadata['resolve']['nodes']}
        active, pending = set(), [metadata['resolve']['root']]
        while pending:
            current = pending.pop()
            if current not in active:
                active.add(current)
                pending.extend(nodes[current]['dependencies'])
        packages = [p for p in metadata['packages'] if p['id'] in active and p['name'] != 'suisuiview']
        missing = {(p['name'], p['version']) for p in packages} - covered
        if missing:
            raise RuntimeError(f'Packages missing from generated license report: {sorted(missing)}')
        inventory = []
        for package in packages:
            base = Path(package['manifest_path']).parent
            key = f"{package['name']}-{package['version']}"
            files = []
            for file in sorted(base.rglob('*')):
                if file.is_file() and (
                    re.match(r'^(license|licence|copying|copyright|notice|patents|authors)([._-]|$)', file.name, re.I)
                    or 'licenses' in {p.lower() for p in file.relative_to(base).parts[:-1]}
                ):
                    relative = file.relative_to(base)
                    dest = bundle / 'licenses' / key / relative
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(file, dest)
                    files.append(relative.as_posix())
            inventory.append({'name': package['name'], 'version': package['version'],
                              'license': package['license'], 'source': package['source'],
                              'notice_files': files})
        for filename in ('LICENSE', 'THIRD_PARTY_NOTICES.txt', 'README.md'):
            shutil.copyfile(ROOT / filename, bundle / filename)
        shutil.copytree(ROOT / 'licenses', bundle / 'licenses', dirs_exist_ok=True)
        # Cargo reports exclude the standard library linked by rustc.
        rust_docs = Path(run('rustc', '--print', 'sysroot').strip()) / 'share/doc/rust'
        rust_licenses = bundle / 'licenses/rust-standard-library'
        rust_licenses.mkdir(parents=True)
        for filename in ('COPYRIGHT.html', 'LICENSE-APACHE', 'LICENSE-MIT'):
            shutil.copyfile(rust_docs / filename, rust_licenses / filename)
        if (rust_docs / 'COPYRIGHT-library.html').exists():
            shutil.copyfile(rust_docs / 'COPYRIGHT-library.html', rust_licenses / 'COPYRIGHT-library.html')
        shutil.copyfile(exe, bundle / 'suisuiview.exe')
        (bundle / 'THIRD_PARTY_LICENSES.txt').write_text(licenses, encoding='utf-8')
        (bundle / 'dependency-inventory.json').write_text(json.dumps(inventory, indent=2), encoding='utf-8')
        source_archive = dist / f'{source_name}.tar.gz'
        source_instructions = (
            f'SuiSuiView {version}\nSource commit: {commit}\n'
            f'https://github.com/BK927/SuiSuiView/tree/{commit}\n\n'
            f'Corresponding Source: {source_archive.name}, supplied alongside the binary ZIP.\n'
            'Includes Cargo.lock, patched winit, native library sources, build scripts,\n'
            'and vendored Cargo dependencies. See LICENSE for GPL-3.0-only terms.\n'
            'Extract with tar -xf. Install Rust 1.99.1 (MSVC), VS C++ tools and a Windows SDK.\n'
            'In the extracted source directory, build without fetching crates:\n'
            "$env:RUSTFLAGS='-C target-feature=+crt-static'\n"
            'cargo +1.99.1 build --release --frozen --target x86_64-pc-windows-msvc --bin suisuiview\n'
            'The compiler and Windows SDK are separate build prerequisites.\n'
        )
        (source / 'SOURCE_BUILD.txt').write_text(source_instructions, encoding='utf-8')
        (bundle / 'SOURCE.txt').write_text(source_instructions, encoding='utf-8')
        (bundle / 'START_HERE.txt').write_text(
            'SuiSuiView for Windows x64\n\n'
            'Extract the entire ZIP to a user-writable folder, then open suisuiview.exe.\n'
            'Windows 10/11 x64 with a working OpenGL graphics driver is the intended target.\n'
            'Open files with Ctrl+O or drag and drop; open a folder with F; settings with F5.\n'
            'Keep LICENSE, THIRD_PARTY_NOTICES.txt, THIRD_PARTY_LICENSES.txt and licenses/.\n'
            'No installer, administrator access, Rust, Python or .NET runtime is required.\n'
            'This free build has no automatic updater. It is currently unsigned.\n'
            'Windows may show an unknown-publisher/SmartScreen warning. Do not disable protection.\n'
            'AVIF, PDFium/AI previews and external model weights are not included.\n'
            'Known issue: LZMA-compressed ZIP/CBZ fails to decode in the current zip dependency.\n'
            'Use Stored/Deflate/Bzip2 archives for this candidate. Public release is blocked.\n'
            'Settings are stored in the user AppData SuiSuiView directory.\n'
            'Build/test success does not certify a clean user PC or graphics compatibility.\n'
            'See release-manifest.json for the exact tested source and validation limits.\n',
            encoding='utf-8',
        )
        with tarfile.open(source_archive, 'w:gz') as tar:
            tar.add(source, arcname=source_name)
        manifest = {
            'version': version, 'commit': commit, 'target': TARGET,
            'features': ['default', 'native-webp'], 'rustflags': os.environ['RUSTFLAGS'],
            'rustc': run('rustc', '-Vv').strip(), 'cargo': run('cargo', '-V').strip(),
            'cargo_lock_sha256': sha256(ROOT / 'Cargo.lock'),
            'executable_sha256': sha256(exe), 'dll_imports': imports,
            'source_archive': source_archive.name, 'source_sha256': sha256(source_archive),
            'ci_run': os.environ.get('GITHUB_RUN_ID'), 'signed': False,
            'release_ready': False,
            'compatibility_probes': compatibility,
            'validation': {'regression_tests': 'passed (see cargo-test.txt for ignored cases)',
                           'decoder_archive_smoke': '8 required cases / 48 decoded pages',
                           'lzma_zip': 'See compatibility_probes; failures are NOT passes',
                           'source_metadata_offline': True, 'gui': 'not certified by packaging',
                           'clean_user_pc': 'NOT TESTED', 'gpu_hardware_matrix': 'NOT TESTED'},
        }
        (bundle / 'release-manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
        shutil.copyfile(validation / 'build-receipt.json', bundle / 'build-receipt.json')
        (dist / 'release-manifest.json').write_text(json.dumps(manifest, indent=2), encoding='utf-8')
        binary_archive = dist / f'{name}.zip'
        with zipfile.ZipFile(binary_archive, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            for file in sorted(bundle.rglob('*')):
                if file.is_file():
                    z.write(file, file.relative_to(stage).as_posix())
        (dist / 'SHA256SUMS.txt').write_text(''.join(
            f'{sha256(p)}  {p.name}\n' for p in (binary_archive, source_archive)
        ), encoding='ascii')
        print(f'Created {binary_archive.name} and {source_archive.name}')


if __name__ == '__main__':
    main()
