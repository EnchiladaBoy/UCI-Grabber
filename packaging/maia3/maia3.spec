# PyInstaller one-directory specification for the separately distributed,
# model-free Maia3 component. build_runtime.py creates the three launch names.

from PyInstaller.utils.hooks import collect_data_files, copy_metadata


datas = collect_data_files("maia3")
datas += copy_metadata("maia3")
datas += copy_metadata("chess")
datas += copy_metadata("numpy")
datas += copy_metadata("torch")

analysis = Analysis(
    ["maia3_entry.py"],
    pathex=[],
    binaries=[],
    datas=datas,
    hiddenimports=["chess", "numpy", "torch"],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=["huggingface_hub", "pip", "setuptools", "tkinter"],
    noarchive=False,
    optimize=1,
)
pyz = PYZ(analysis.pure)
executable = EXE(
    pyz,
    analysis.scripts,
    [],
    exclude_binaries=True,
    name="maia3-engine",
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,
    console=True,
)
collection = COLLECT(
    executable,
    analysis.binaries,
    analysis.datas,
    strip=False,
    upx=False,
    upx_exclude=[],
    name="maia3-engine",
)
