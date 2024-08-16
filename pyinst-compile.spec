# -*- mode: python ; coding: utf-8 -*-
import os
import site

# Locate the site-packages directory
site_packages_path = next(p for p in site.getsitepackages() if 'site-packages' in p)

# Path to the legacy_blocks.json file
legacy_blocks_path = os.path.join(site_packages_path, 'anvil', 'legacy_blocks.json')

block_cipher = None

a = Analysis(['arnis.py'],
             pathex=['.'],
             binaries=[],
             datas=[(legacy_blocks_path, 'anvil')],
             hiddenimports=[],
             hookspath=[],
             runtime_hooks=[],
             excludes=[],
             cipher=block_cipher,
             noarchive=False)
pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)
exe = EXE(pyz,
          a.scripts,
          a.binaries,
          a.zipfiles,
          a.datas,
          name='arnis',
          debug=False,
          strip=False,
          upx=True,
          runtime_tmpdir=None,
          console=True )
