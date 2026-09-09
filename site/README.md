# Public introduction site

The four language pages are generated from one HTML template, local CSS and
complete language files. Python 3.10 or later is enough; no packages are needed.

```text
python -m unittest discover -s site -p "test_*.py"
python site/build.py
python site/build.py --check
```

The output is `site/_build/`, which is ignored by Git. The public URL is
`https://bk927.github.io/SuiSuiView/`; root-relative paths intentionally include
the GitHub Pages project name. For local browser review, serve the output under
that same `/SuiSuiView/` path.

`build.py` copies only the named screenshots and optimized icon in `assets/site/`.
It never copies a repository directory wholesale. Validation rejects
extra deployed files, missing assets, broken local links and anchors, incorrect
language metadata and image dimensions, external page dependencies, and active
content. Existing unexpected output files, symlinks and Windows junctions stop
the build before any output is written; the build never deletes them. All
locale files must have the same content structure.

The Pages workflow runs these checks before uploading the generated directory.
Only changes to the site, its public images, icon or workflow on `main`, and
manual runs, trigger it. The deployment job runs only from `main`.

When release availability changes, update every locale together. Keep the
free GitHub route visible when adding a paid Store route. Add executable or
Store links only after the corresponding release is publicly available.
