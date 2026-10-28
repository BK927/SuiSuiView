"""Build a per-user MSI from the already validated ZIP. Requires wixl 0.106."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import uuid
import xml.etree.ElementTree as ET
import zipfile

UPGRADE_CODE = '05d21634-622c-4cf2-9346-af977ce75b87'
NAMESPACE = uuid.UUID(UPGRADE_CODE)
APP_ID = 'SuiSuiView.Installed'
INSTALLER_KEY = r'Software\BK927\SuiSuiView\Installer'
CAPABILITIES = rf'Software\BK927\SuiSuiView\Associations\{APP_ID}\Capabilities'


def identity(prefix: str, value: str) -> str:
    return prefix + hashlib.sha256(value.encode()).hexdigest()[:28]


def guid(value: str) -> str:
    return str(uuid.uuid5(NAMESPACE, value)).upper()


def node(parent, tag, **attrs):
    return ET.SubElement(parent, tag, {k: str(v) for k, v in attrs.items()})


def sql_string(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def sql(msi: Path, query: str) -> None:
    subprocess.run(['msibuild', str(msi), '-q', query], check=True)


def add_ui(product):
    # First-party UI: no WiX UI extension, DLL or third-party artwork is bundled.
    ui = node(product, 'UI', Id='SuiSuiViewUI')
    node(ui, 'TextStyle', Id='Normal', FaceName='Segoe UI', Size='9')
    node(ui, 'Property', Id='DefaultUIFont', Value='Normal')
    def dialog(name, message, button, event, value):
        d = node(ui, 'Dialog', Id=name, Width=370, Height=180, Title='SuiSuiView Setup')
        node(d, 'Control', Id='Message', Type='Text', X=20, Y=25, Width=330, Height=85, Text=message)
        c = node(d, 'Control', Id='Continue', Type='PushButton', X=260, Y=145, Width=90, Height=20,
                 Text=button, Default='yes')
        node(c, 'Publish', Event=event, Value=value).text = '1'
        return d
    d = dialog('WelcomeDlg', 'Install SuiSuiView for your Windows account.\n\n'
               'No administrator access is required. Existing default apps are preserved.\n'
               'This free build has no automatic updater.', 'Install', 'EndDialog', 'Return')
    c = node(d, 'Control', Id='Cancel', Type='PushButton', X=160, Y=145, Width=90, Height=20,
             Text='Cancel', Cancel='yes')
    node(c, 'Publish', Event='EndDialog', Value='Exit').text = '1'
    dialog('ExitDlg', 'SuiSuiView setup completed. Open SuiSuiView from the Start menu.\n\n'
           'Choose default file types in Windows Settings when needed.', 'Finish', 'EndDialog', 'Return')
    dialog('ErrorDlg', 'Setup did not complete. No successful installation is claimed.\n'
           'Use the Windows Installer log to diagnose the failure.', 'Close', 'EndDialog', 'Return')
    dialog('CancelDlg', 'Setup was cancelled.', 'Close', 'EndDialog', 'Return')
    seq = node(ui, 'InstallUISequence')
    node(seq, 'Show', Dialog='WelcomeDlg', Before='ExecuteAction').text = 'NOT Installed'
    node(seq, 'Show', Dialog='ExitDlg', OnExit='success').text = 'NOT REMOVE'
    node(seq, 'Show', Dialog='ErrorDlg', OnExit='error').text = '1'
    node(seq, 'Show', Dialog='CancelDlg', OnExit='cancel').text = '1'


def make_xml(payload: Path, manifest: dict, version: str, destination: Path) -> tuple[str, list[tuple[str, str]]]:
    if not re.fullmatch(r'\d+\.\d+\.\d+', version):
        raise ValueError('MSI requires a three-part numeric version')
    major, minor, build = map(int, version.split('.'))
    if major > 255 or minor > 255 or build > 65535:
        raise ValueError('Version exceeds Windows Installer limits')
    product_code = guid('product:' + version)
    wix = ET.Element('Wix', xmlns='http://schemas.microsoft.com/wix/2006/wi')
    product = node(wix, 'Product', Id=product_code, Name='SuiSuiView', Language='1033',
                   Version=version, Manufacturer='BK927', UpgradeCode=UPGRADE_CODE)
    node(product, 'Package', Id='*', InstallerVersion='500', Compressed='yes', InstallScope='perUser',
         Description='SuiSuiView Windows x64', Manufacturer='BK927')
    node(product, 'MajorUpgrade', DowngradeErrorMessage='A newer SuiSuiView is already installed.')
    node(product, 'MediaTemplate', EmbedCab='yes')
    node(product, 'Property', Id='ARPNOMODIFY', Value='1')
    node(product, 'Property', Id='ARPURLINFOABOUT', Value='https://github.com/BK927/SuiSuiView')
    node(product, 'Condition', Message='64-bit Windows is required.').text = 'VersionNT64'
    node(product, 'Condition', Message='This package installs for the current user only.').text = 'NOT ALLUSERS'
    root = node(product, 'Directory', Id='TARGETDIR', Name='SourceDir')
    local = node(root, 'Directory', Id='LocalAppDataFolder')
    programs = node(local, 'Directory', Id='UserProgramsDir', Name='Programs')
    install = node(programs, 'Directory', Id='INSTALLDIR', Name='SuiSuiView')
    menu = node(node(root, 'Directory', Id='ProgramMenuFolder'), 'Directory', Id='MenuDir', Name='SuiSuiView')
    feature = node(product, 'Feature', Id='Application', Title='SuiSuiView', Level='1')
    directories = {'': install}
    def directory(relative):
        if relative not in directories:
            path = Path(relative)
            directories[relative] = node(directory(path.parent.as_posix() if path.parent != Path('.') else ''),
                                         'Directory', Id=identity('dir', relative), Name=path.name)
        return directories[relative]
    for file in sorted(payload.rglob('*')):
        if not file.is_file():
            continue
        relative = file.relative_to(payload).as_posix()
        parent = file.parent.relative_to(payload).as_posix()
        comp_id = identity('cmp', relative)
        component = node(directory('' if parent == '.' else parent), 'Component', Id=comp_id,
                         Guid=guid('file:' + relative), Win64='yes')
        node(component, 'File', Id=identity('file', relative), Source=str(file), Name=file.name, KeyPath='yes')
        node(feature, 'ComponentRef', Id=comp_id)
    component = node(install, 'Component', Id='Registration', Guid=guid('registration'), Win64='yes')
    node(feature, 'ComponentRef', Id='Registration')
    index = 0
    def reg(key, name, value, key_path=False):
        nonlocal index
        index += 1
        attrs = dict(Id=f'reg{index}', Root='HKCU', Key=key, Type='string', Value=value)
        if name:
            attrs['Name'] = name
        if key_path:
            attrs['KeyPath'] = 'yes'
        node(component, 'RegistryValue', **attrs)
    reg(INSTALLER_KEY, 'InstallLocation', '[INSTALLDIR]', True)
    reg(INSTALLER_KEY, 'ProductVersion', version)
    reg(INSTALLER_KEY, 'ProductCode', product_code)
    reg(CAPABILITIES, 'ApplicationName', 'SuiSuiView')
    reg(CAPABILITIES, 'ApplicationDescription', 'Image and comic viewer')
    reg(CAPABILITIES, 'ApplicationIcon', '"[INSTALLDIR]suisuiview.exe",0')
    reg(CAPABILITIES, 'OwnedExecutable', '[INSTALLDIR]suisuiview.exe')
    reg(r'Software\RegisteredApplications', APP_ID, CAPABILITIES)
    known = set(manifest['known_extensions'])
    defaults = set(manifest['default_extensions'])
    if not defaults <= known or '.zip' in defaults or '.cbz' not in defaults:
        raise ValueError('Unexpected verified file association defaults')
    cleanup = []
    for extension in sorted(known):
        if not re.fullmatch(r'\.[a-z0-9_]+', extension):
            raise ValueError('Invalid file extension in validated manifest')
        prog_id = APP_ID + extension
        class_key = 'Software\\Classes\\' + prog_id
        # Cleanup covers dynamically selected formats too, without touching
        # portable registrations or another application's OpenWith entry.
        cleanup.extend([(class_key, '-'), (rf'Software\Classes\{extension}\OpenWithProgids', prog_id)])
        if extension not in defaults:
            continue
        reg(CAPABILITIES + r'\FileAssociations', extension, prog_id)
        reg(rf'Software\Classes\{extension}\OpenWithProgids', prog_id, '')
        reg(class_key, '', f'SuiSuiView {extension[1:]} file')
        reg(class_key, 'FriendlyTypeName', f'SuiSuiView {extension[1:]} file')
        reg(class_key + r'\DefaultIcon', '', '"[INSTALLDIR]suisuiview.exe",0')
        reg(class_key + r'\shell\open\command', '', '"[INSTALLDIR]suisuiview.exe" "%1"')
    cleanup.extend([(CAPABILITIES.rsplit('\\', 1)[0], '-'), (INSTALLER_KEY, '-')])
    shortcut = node(menu, 'Component', Id='StartMenu', Guid=guid('start-menu'), Win64='yes')
    node(feature, 'ComponentRef', Id='StartMenu')
    node(shortcut, 'Shortcut', Id='Launch', Name='SuiSuiView', Target='[INSTALLDIR]suisuiview.exe',
         WorkingDirectory='INSTALLDIR')
    node(shortcut, 'RegistryValue', Root='HKCU', Key=INSTALLER_KEY, Name='StartMenu', Value='1', Type='integer', KeyPath='yes')
    node(shortcut, 'RemoveFolder', Id='RemoveMenu', On='uninstall')
    node(product, 'UIRef', Id='SuiSuiViewUI')
    add_ui(node(wix, 'Fragment'))
    ET.indent(wix)
    ET.ElementTree(wix).write(destination, encoding='utf-8', xml_declaration=True)
    return product_code, cleanup


def build(payload: Path, associations: dict, version: str, output: Path, fail=False):
    xml = output.with_suffix('.wxs')
    product_code, cleanup = make_xml(payload, associations, version, xml)
    # Enable wixl's UI table support with first-party empty extension entrypoints;
    # do not import its MS-RL dialogs, strings, bitmaps or DLLs into our MSI.
    with tempfile.TemporaryDirectory(prefix='msi-ui-', dir=output.parent) as temp:
        ext = Path(temp) / 'ui'
        ext.mkdir()
        for name in ('Common', 'CancelDlg'):
            (ext / f'{name}.wxs').write_text('<Wix xmlns="http://schemas.microsoft.com/wix/2006/wi"><Fragment /></Wix>')
        subprocess.run(['wixl', '--ext', 'ui', '--extdir', temp, '-a', 'x64', '-o', str(output), str(xml)], check=True)
    # wixl schedules removal before the transaction by default. Move it after
    # InstallInitialize so a failed upgrade can roll back the prior product.
    sql(output, 'UPDATE `InstallExecuteSequence` SET `Sequence` = 1501 WHERE `Action` = \'RemoveExistingProducts\'')
    # Registry table "-" rows remove only our dedicated subtrees on uninstall.
    # Name-specific NULL rows remove our optional OpenWith values on uninstall.
    for index, (key, name) in enumerate(cleanup):
        sql(output, 'INSERT INTO `Registry` (`Registry`, `Root`, `Key`, `Name`, `Component_`) VALUES '
            f"('cleanup{index}', 1, {sql_string(key)}, {sql_string(name)}, 'Registration')")
    if fail:
        sql(output, 'INSERT INTO `CustomAction` (`Action`, `Type`, `Target`) VALUES '
            "('TestRollbackFailure', 19, 'Intentional disposable-runner rollback test')")
        sql(output, 'INSERT INTO `InstallExecuteSequence` (`Action`, `Condition`, `Sequence`) VALUES '
            "('TestRollbackFailure', 'NOT Installed', 6499)")
    # Extracting the MSI and comparing every payload byte catches missing CAB
    # entries and accidental harvest omissions before the Windows install job.
    with tempfile.TemporaryDirectory(prefix='msi-extract-', dir=output.parent) as temp:
        subprocess.run(['msiextract', '-C', temp, str(output)], check=True, stdout=subprocess.DEVNULL)
        extracted = Path(temp)
        for file in payload.rglob('*'):
            if file.is_file():
                relative = file.relative_to(payload)
                matches = [p for p in extracted.rglob(file.name) if p.as_posix().endswith(relative.as_posix())]
                if len(matches) != 1 or matches[0].read_bytes() != file.read_bytes():
                    raise RuntimeError(f'MSI payload differs: {relative}')
    xml.unlink()
    return product_code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--zip', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--test-fixtures', type=Path)
    args = parser.parse_args()
    if subprocess.check_output(['wixl', '--version'], text=True).strip() != '0.106':
        raise RuntimeError('Use pinned wixl 0.106')
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='msi-stage-', dir=args.output.resolve()) as temp:
        stage = Path(temp)
        with zipfile.ZipFile(args.zip) as archive:
            for info in archive.infolist():
                path = Path(info.filename)
                if path.is_absolute() or '..' in path.parts or '\\' in info.filename or ':' in info.filename:
                    raise ValueError('Unsafe ZIP path')
            archive.extractall(stage)
        roots = list(stage.iterdir())
        if len(roots) != 1 or not roots[0].is_dir():
            raise ValueError('Expected exactly one payload directory')
        payload = roots[0]
        manifest = json.loads((payload / 'release-manifest.json').read_text())
        receipt = json.loads((payload / 'build-receipt.json').read_text(encoding='utf-8-sig'))
        if receipt['commit'] != manifest['commit'] or receipt['regression_tests'] != 'passed':
            raise ValueError('MSI input is not the validated build')
        exe_hash = hashlib.sha256((payload / 'suisuiview.exe').read_bytes()).hexdigest()
        if exe_hash != manifest['executable_sha256']:
            raise ValueError('MSI executable differs from tested executable')
        associations = json.loads((payload / 'file-associations.json').read_text())
        version = manifest['version']
        output = args.output.resolve() / f'SuiSuiView-{version}-windows-x64.msi'
        code = build(payload, associations, version, output)
        evidence = {'commit': manifest['commit'], 'version': version, 'product_code': code,
                    'upgrade_code': UPGRADE_CODE, 'installer': output.name, 'per_user': True,
                    'signed': False, 'payload_matches_zip': True, 'clean_pc_tested': False,
                    'sha256': hashlib.sha256(output.read_bytes()).hexdigest()}
        (args.output / 'msi-manifest.json').write_text(json.dumps(evidence, indent=2))
        if args.test_fixtures:
            args.test_fixtures.mkdir(parents=True, exist_ok=True)
            # Test versions are never release assets and share the UpgradeCode.
            major, minor, patch = map(int, version.split('.'))
            older = f'{major}.{minor-1}.0' if patch == 0 and minor else f'{major}.{minor}.{max(0, patch-1)}'
            if older == version:
                raise ValueError('No lower test version available')
            build(payload, associations, older, args.test_fixtures.resolve() / 'older.msi')
            build(payload, associations, f'{major}.{minor}.{patch+1}', args.test_fixtures.resolve() / 'rollback.msi', fail=True)
            (args.test_fixtures / 'versions.json').write_text(json.dumps({'older': older, 'current': version}))


if __name__ == '__main__':
    main()
