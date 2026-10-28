import tempfile
import unittest
from pathlib import Path
import xml.etree.ElementTree as ET
from build_msi import make_xml, APP_ID, UPGRADE_CODE


class InstallerAuthoring(unittest.TestCase):
    def test_payload_identity_per_user_registry_and_portable_isolation(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            payload = root / 'payload'
            payload.mkdir()
            (payload / 'suisuiview.exe').write_bytes(b'fixture')
            (payload / 'LICENSE').write_text('fixture')
            association = {'known_extensions': ['.png', '.cbz', '.zip'], 'default_extensions': ['.png', '.cbz']}
            paths = [root / 'old.wxs', root / 'new.wxs']
            old_code, _ = make_xml(payload, association, '0.1.0', paths[0])
            new_code, cleanup = make_xml(payload, association, '0.1.1', paths[1])
            old, new = [ET.parse(p) for p in paths]
            ns = {'w': 'http://schemas.microsoft.com/wix/2006/wi'}
            self.assertNotEqual(old_code, new_code)
            self.assertEqual(old.find('w:Product', ns).get('UpgradeCode'), UPGRADE_CODE)
            self.assertEqual(old.find('.//w:Package', ns).get('InstallScope'), 'perUser')
            self.assertEqual([e.get('Guid') for e in old.findall('.//w:Component', ns)],
                             [e.get('Guid') for e in new.findall('.//w:Component', ns)])
            registry = old.findall('.//w:RegistryValue', ns)
            self.assertTrue(all(e.get('Root') == 'HKCU' for e in registry))
            self.assertTrue(all('UserChoice' not in e.get('Key') for e in registry))
            self.assertTrue(all('Portable' not in key for key, _ in cleanup))
            self.assertIn((rf'Software\Classes\.zip\OpenWithProgids', APP_ID + '.zip'), cleanup)
            self.assertFalse(any(e.get('Key').endswith('.zip\\shell\\open\\command') for e in registry))
            self.assertEqual(len(old.findall('.//w:File', ns)), 2)

    def test_invalid_version_or_extension_cannot_generate_installer(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for version in ('0.1.0-alpha', '256.0.0', '1.0.65536'):
                with self.assertRaises(ValueError):
                    make_xml(root, {}, version, root / 'test.wxs')
            with self.assertRaises(ValueError):
                make_xml(root, {'known_extensions': ['.cbz', '.bad\\key'],
                               'default_extensions': ['.cbz']}, '0.1.0', root / 'test.wxs')
