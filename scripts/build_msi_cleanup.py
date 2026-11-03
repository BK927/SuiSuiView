"""Build and audit the first-party MSI transaction helper on Windows/MSVC."""
import json
import os
from pathlib import Path
import subprocess
from package_windows import ROOT, SYSTEM_DLLS, check_imports, dumpbin, run, sha256

out = ROOT / 'target/release-validation/msi-tools'
out.mkdir(parents=True, exist_ok=True)
vswhere = Path(os.environ['ProgramFiles(x86)']) / 'Microsoft Visual Studio/Installer/vswhere.exe'
install = Path(run(str(vswhere), '-latest', '-products', '*', '-requires',
                   'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationPath').strip())
dll = out / 'msi-cleanup.dll'
batch = out / 'build.cmd'
batch.write_text('@echo off\ncall "' + str(install / 'VC/Auxiliary/Build/vcvars64.bat') + '"\n'
                 'if errorlevel 1 exit /b 1\n'
                 f'cl /nologo /LD /MT /O2 /W4 /Fe"{dll}" /Fo"{out / "msi-cleanup.obj"}" '
                 f'"{ROOT / "scripts/msi_cleanup.c"}" msi.lib /link /IMPLIB:"{out / "msi-cleanup.lib"}"\n',
                 encoding='utf-8')
subprocess.run(['cmd.exe', '/d', '/c', str(batch)], cwd=out, check=True)
imports = check_imports(run(str(dumpbin()), '/dependents', str(dll)), SYSTEM_DLLS | {'msi'})
(out / 'msi-cleanup.json').write_text(json.dumps({'commit': run('git', 'rev-parse', 'HEAD').strip(),
    'sha256': sha256(dll), 'imports': imports, 'license': 'GPL-3.0-only'}, indent=2), encoding='utf-8')
