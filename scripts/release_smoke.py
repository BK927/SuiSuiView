"""Exercise real default decoders and ZIP backends; this is not a GUI test."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import zipfile


def verify_report(report: dict, expected: int) -> None:
    if report.get('page_count') != expected or report.get('failures') != 0:
        raise RuntimeError('Wrong page count or failed decoder in smoke report')
    pages = report.get('pages', [])
    if len(pages) != expected or any(
        p.get('error') or not p.get('display_bytes') or not p.get('decode_backend')
        for p in pages
    ):
        raise RuntimeError('Missing decoded pixels or per-page evidence')


def main() -> None:
    from PIL import Image, ImageDraw

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--cli', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    cli, output = args.cli.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    # A fresh child directory avoids mixing reports or old fixtures on reruns.
    import tempfile
    fixture = Path(tempfile.mkdtemp(prefix='fixture-', dir=output))
    book = fixture / '한글 폴더 with spaces'
    book.mkdir()
    im = Image.new('RGB', (640, 960), 'white')
    draw = ImageDraw.Draw(im)
    draw.rectangle((40, 50, 600, 430), fill=(20, 70, 180))
    draw.ellipse((100, 500, 540, 900), fill=(240, 130, 20))
    for i, fmt in enumerate(('PNG', 'JPEG', 'WEBP', 'BMP', 'GIF', 'TIFF')):
        im.save(book / f'{i + 1:02d}-페이지.{fmt.lower()}', format=fmt)
    sources = [('folder', book)]
    for name, compression in (
        ('stored', zipfile.ZIP_STORED), ('deflate', zipfile.ZIP_DEFLATED),
        ('bzip2', zipfile.ZIP_BZIP2), ('lzma', zipfile.ZIP_LZMA),
    ):
        archive = fixture / f'만화 {name}.cbz'
        with zipfile.ZipFile(archive, 'w', compression=compression) as z:
            for file in sorted(book.iterdir()):
                z.write(file, f'책/{file.name}')
        sources.append((name, archive))
    env = os.environ.copy()
    env['SUISUIVIEW_PROFILE_DIR'] = str(fixture / 'profile')
    results = []
    for strategy in ('auto', 'image-crate'):
        for name, source in sources:
            report_path = output / f'{name}-{strategy}.json'
            subprocess.run(
                [str(cli), '--perf-scan', str(source), '--decode-strategy', strategy,
                 '--target-long-edge', '384', '--perf-report', str(report_path)],
                check=True, env=env, timeout=120,
            )
            report = json.loads(report_path.read_text(encoding='utf-8'))
            verify_report(report, 6)
            results.append({'case': f'{name}-{strategy}', 'pages': 6,
                            'backend_counts': report['backend_counts']})
    (output / 'summary.json').write_text(
        json.dumps({'passed': True, 'cases': results, 'gui_tested': False,
                    'clean_pc_tested': False}, indent=2), encoding='utf-8',
    )


if __name__ == '__main__':
    main()
