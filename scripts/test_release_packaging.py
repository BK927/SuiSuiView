import unittest

from package_windows import check_imports, license_report
from release_smoke import verify_report


class PackagingGates(unittest.TestCase):
    def test_system_imports_and_delay_imports(self):
        self.assertEqual(check_imports('    KERNEL32.dll\n    api-ms-win-core-synch-l1-2-0.dll\n'),
                         ['KERNEL32.dll', 'api-ms-win-core-synch-l1-2-0.dll'])
        for dll in ('VCRUNTIME140.dll', 'MSVCP140.dll', 'pdfium.dll', 'libwebp.dll'):
            with self.subTest(dll=dll), self.assertRaises(RuntimeError):
                check_imports(f'    KERNEL32.dll\n    {dll}\n')
        with self.assertRaises(RuntimeError):
            check_imports('dumpbin failed')

    def test_decoder_exit_success_is_not_enough(self):
        good = {'page_count': 1, 'failures': 0, 'pages': [
            {'error': None, 'display_bytes': 64, 'decode_backend': 'libwebp-scaled'}]}
        verify_report(good, 1)
        for bad in ({**good, 'failures': 1}, {**good, 'pages': []},
                    {**good, 'page_count': 0},
                    {**good, 'pages': [{'error': 'decode failed', 'display_bytes': None}]}):
            with self.assertRaises(RuntimeError):
                verify_report(bad, 1)

    def test_notices_retain_crate_identity_and_full_text(self):
        data = {'licenses': [{'name': 'MIT License', 'id': 'MIT', 'text': 'Copyright Example\nPermission...',
                              'used_by': [{'crate': {'name': 'example', 'version': '1.0.0', 'repository': None}}]}]}
        text, covered = license_report(data)
        self.assertIn('Copyright Example\nPermission...', text)
        self.assertEqual(covered, {('example', '1.0.0')})
        with self.assertRaises(RuntimeError):
            license_report({'licenses': []})


if __name__ == '__main__':
    unittest.main()
