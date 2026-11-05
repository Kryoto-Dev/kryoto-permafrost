# The .kryo format

Everything here is in `crates/kryo/src/format.rs`. Numbers are little-endian.

## Files

A repack is one of these:

- **One setup.** The data sits inside the `.exe`, after the program.
- **A setup with `.kryo` files beside it.** The main game is `Name.kryo`, an optional part
  is `Name.<part>.kryo`, and split sets are numbered: `Name.001.kryo`, `Name.002.kryo`.

Windows will not start an `.exe` of 4 GiB or more, so a repack past 3.9 GB always keeps its
data beside the setup.

Each `.kryo` opens with 32 bytes:

| Offset | Size | What |
|---|---|---|
| 0 | 4 | `KRYO` |
| 4 | 2 | format version |
| 8 | 16 | set id, so a part from another repack is caught |
| 24 | 4 | part number, from 1 |

Data follows. A block can run from one part into the next.

The setup, and the last part of the main set, end with a trailer of 56 bytes:

| Offset | Size | What |
|---|---|---|
| 0 | 8 | where the setup's art starts |
| 8 | 8 | its length |
| 16 | 8 | where the manifest starts |
| 24 | 8 | its length |
| 32 | 16 | first half of the manifest's BLAKE3 |
| 48 | 8 | `KRYOTAIL` |

The manifest is a [postcard](https://postcard.jamesmunns.com) `Manifest`, compressed with
Zstandard. It lists the files, the parts, and every block.

## Blocks

A block is compressed on its own, so blocks decode side by side. The manifest gives each one
its codec and the BLAKE3 of its packed bytes.

| Codec | Used for |
|---|---|
| Store | JPEGs after Lepton, and media that will not shrink |
| Zstandard | everything, in Frost |
| LZMA2 | game data |
| x86 filter + LZMA2 | native programs and libraries |
| Delta filter + LZMA2 | PCM audio, with the distance set to one sample frame |
| PPMd | text, where a sample shows it beats LZMA2 |

A block's decoded stream is a run of entries. An entry says which file its bytes belong to,
where in the file they go, and whether they are raw or wrapped.

## Wrapped entries

Deflate data looks random to a compressor, so a PNG or a zipped asset would go through almost
untouched. A wrapped entry holds the data opened up, plus what it takes to rebuild the original
bit for bit:

```text
0 raw      varint len, bytes
1 deflate  varint corrections len, varint plain len, corrections, plain
2 png      varint chunk count, chunk sizes, 2 header bytes, varint tail len, tail,
           then a deflate item's body
3 jpeg     varint len, Lepton bytes
```

Corrections come from [preflate](https://github.com/microsoft/preflate-rs), which works out how
the stream was made. JPEGs go through [Lepton](https://github.com/microsoft/lepton_jpeg_rust).
Both rebuild each item during the freeze, and anything that does not come back the same is
stored as it was.

## Repeated data

Files are cut into chunks of about 32 KiB by their content, so an inserted byte only moves one
boundary. A chunk seen before is not stored. The file records a reference instead:

```text
offset in this file, source file, offset in the source, length
```

Whole duplicate files are marked `copy_of`. Both are filled in after every block is written, by
copying from what is already on the disk, so they cost no memory however far apart they are.

A reference only ever points at bytes that are stored, so the order of the copies does not
matter. It only ever points into the same part or the main game, so an optional part that was
not downloaded cannot break anything.

## Installing

1. Check that the parts for the selection are there and long enough.
2. Create the files at full size.
3. Decode blocks in parallel, as far as memory allows, and check each one's hash.
   Finished blocks go in `.kryo-thaw`, so a stopped install carries on.
4. Fill in references and copies.
5. Hash every file against the manifest, if asked to.
6. Restore modified times and the read-only flag.
