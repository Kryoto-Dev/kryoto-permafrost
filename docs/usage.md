# Using Permafrost

## Making a repack

1. Drop the game folder on the window.
2. Press **Find** to fill in the title, version, program and cover from Kryoto. For a game that
   isn't on Kryoto, fill them in yourself.
3. Pick a mode. Run **Frost** first if you're not sure about a game: it takes minutes, and the
   setup it makes works the same.
4. Press **Estimate size** to see roughly where it will land.
5. Press **Freeze**.

You can pause at any time. **Stop** keeps the finished blocks: freeze the same folder with the
same settings and it picks up from them. If a freeze is stopped for good, delete the hidden
`.permafrost-...` folder it left in the output folder.

### Modes

| Mode | Compression | Good for |
|---|---|---|
| Frost | Zstandard | testing a repack |
| Deep Freeze | LZMA2 and every filter | most games |
| Absolute Zero | LZMA2 with a dictionary up to 1 GiB, settings tuned per block, PPMd for text | the smallest possible size |

**Memory** is what compressing may use. A bigger dictionary finds more, and Absolute Zero takes
about 12 GB per block at full size. With less memory it shrinks the dictionary, or runs fewer
blocks at once. Installing needs far less: the results page tells you how much.

### The output

Up to 3.9 GB the game goes inside the setup, as one file. Past that, or if you ask for it, the
data goes beside the setup as `.kryo` files. Windows will not start an `.exe` of 4 GiB or more.

**Split** cuts the `.kryo` files into parts, for hosts with an upload limit. Every part has to sit
in the same folder as the setup.

**Optional parts** are found by folder: `_CommonRedist` and friends, languages, soundtracks and
artbooks. Each gets its own `.kryo`, so players can skip the download. Leaving a language out only
works if the game copes without it, so test that first.

**Leave out** lists files a game doesn't need: debug symbols, crash dumps, logs.

`Name - repack info.txt` holds the sizes and parts, ready to post with the repack. **Copy info**
puts it on the clipboard.

## Installing one

Run the setup, pick a folder, press **Install**. Keep any `.kryo` files in the same folder as the
setup. Parts you didn't download show as "not downloaded".

- **Check files after** reads every file back and compares it. Turn it off to save time.
- **Use less memory** unpacks one block at a time, for PCs that struggle.
- **Pause** and **Stop** work at any point. Run the setup again, pick the same folder, and it
  carries on.
- A folder Windows protects, like Program Files, brings up one administrator prompt.

To remove a game, use Windows' list of apps, or `Permafrost Uninstall.exe` in the game folder.
Saves and anything else the game wrote stay.

### From a script

```text
"Game Setup.exe" /SILENT /DIR="D:\Games\Game" /NOICONS
```

`/SILENT` installs with no window, and the exit code says how it went. `/MUTE` starts the setup
with the music off.

## From a terminal

```text
permafrost freeze "D:\Games\Game" --out "D:\Repacks" --level zero --memory 16 --by Rainy
permafrost list "D:\Repacks\Game.kryo"
permafrost check "D:\Repacks\Game.kryo"
permafrost thaw "D:\Repacks\Game.kryo" "D:\Games\Game"
```

`permafrost` with no arguments lists every option. Add `--stub <kryoto-setup.exe>` to `freeze` to
build a setup.
