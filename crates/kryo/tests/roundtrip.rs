//! A game comes out of a repack exactly as it went in, whatever the repack did to it.

use kryo::format::{open_repack, Assets, Game, Manifest};
use kryo::freeze::{freeze, Embed, Level, Options};
use kryo::progress::Progress;
use kryo::scan::ComponentRule;
use kryo::thaw::{check_data, missing_volumes, safe_join, thaw, Source, ThawOptions};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Bytes that do not compress, the same every run.
fn noise(len: usize, mut seed: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 56) as u8
        })
        .collect()
}

fn text(len: usize) -> Vec<u8> {
    let line = b"the quick brown fox jumps over the lazy dog, again and again and again\n";
    line.iter().copied().cycle().take(len).collect()
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x9c];
    out.extend(miniz_oxide::deflate::compress_to_vec(data, 6));
    out.extend(adler(data).to_be_bytes());
    out
}

fn adler(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn png(rows: usize) -> Vec<u8> {
    let mut raw = Vec::new();
    for y in 0..rows {
        raw.push(0);
        raw.extend((0..rows * 3).map(|x| ((x * y) % 251) as u8));
    }
    let stream = zlib(&raw);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunk = |kind: &[u8], body: &[u8], out: &mut Vec<u8>| {
        out.extend((body.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend(kind);
        out.extend(body);
        let crc = crc32fast::hash(&out[start..]);
        out.extend(crc.to_be_bytes());
    };
    let mut header = Vec::new();
    header.extend((rows as u32).to_be_bytes());
    header.extend((rows as u32).to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    chunk(b"IHDR", &header, &mut out);
    // Split like real encoders do, so the chunks have to be put back exactly.
    for piece in stream.chunks(8192) {
        chunk(b"IDAT", piece, &mut out);
    }
    chunk(b"IEND", b"", &mut out);
    out
}

fn wav() -> Vec<u8> {
    let samples: Vec<u8> = (0..60_000i32)
        .flat_map(|i| (((i as f32 / 20.0).sin() * 9000.0) as i16).to_le_bytes())
        .collect();
    let mut out = b"RIFF".to_vec();
    out.extend((36 + samples.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(44100u32.to_le_bytes());
    out.extend((44100u32 * 4).to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((samples.len() as u32).to_le_bytes());
    out.extend(samples);
    out
}

/// A folder with one of everything the pipeline treats specially.
fn game(root: &Path) {
    let write = |path: &str, data: &[u8]| {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, data).unwrap();
    };
    let shared = noise(600_000, 1);
    let mut far = noise(90_000, 2);
    far.extend(&shared);
    far.extend(noise(5_000, 3));
    write("game.exe", &[b"MZ".as_slice(), &noise(2000, 4)].concat());
    write("data/first.bin", &shared);
    write("data/second.bin", &far);
    write("data/copy.bin", &shared);
    write("data/empty.dat", b"");
    write("data/notes.txt", &text(300_000));
    write("data/texture.png", &png(200));
    write("data/sound.wav", &wav());
    let mut pak = b"PAK!".to_vec();
    for i in 0..6 {
        pak.extend(zlib(&text(40_000 + i * 977)));
        pak.extend(noise(1000, i as u64));
    }
    write("data/assets.pak", &pak);
    write("video/intro.mp4", &noise(150_000, 9));
    write("lang/french/voice.bin", &text(120_000));
    write("lang/french/voice-copy.bin", &shared);
    fs::create_dir_all(root.join("saves/empty")).unwrap();
}

fn options(source: &Path, out: &Path, level: Level) -> Options {
    Options {
        source: source.to_path_buf(),
        out_dir: out.to_path_buf(),
        name: "Test".into(),
        game: Game { title: "Test".into(), exe: "game.exe".into(), ..Default::default() },
        level,
        memory: 2 << 30,
        threads: 2,
        split: None,
        embed: Embed::Never,
        exclude: Vec::new(),
        components: Vec::new(),
        redists: Vec::new(),
        assets: Assets::default(),
        stub: None,
        test: true,
    }
}

fn contents(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let key = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                out.insert(format!("{key}/"), Vec::new());
                walk(&path, root, out);
            } else {
                out.insert(key, fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn install(manifest: &Manifest, repack: &Path, dest: &Path, selected: Vec<bool>) -> anyhow::Result<()> {
    thaw(
        manifest,
        &ThawOptions {
            dest: dest.to_path_buf(),
            source: Source::new(repack),
            selected,
            threads: 2,
            memory: 1 << 30,
            verify: true,
        },
        &Progress::new(),
    )
}

#[test]
fn every_level_comes_back_identical() {
    for (name, level) in [("frost", Level::Frost), ("deep", Level::DeepFreeze), ("zero", Level::AbsoluteZero)] {
        let dir = scratch(&format!("level-{name}"));
        let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
        game(&source);
        let report = freeze(&options(&source, &out, level), &Progress::new()).unwrap();
        assert!(report.total_output < report.original);
        assert!(report.duplicate_bytes >= 1_200_000, "repeats were missed: {}", report.duplicate_bytes);
        if level != Level::Frost {
            assert!(report.unwrapped_streams >= 7, "only {} streams opened", report.unwrapped_streams);
        }
        let repack = out.join("Test.kryo");
        let (manifest, _) = open_repack(&repack).unwrap();
        install(&manifest, &repack, &dest, vec![true]).unwrap();
        assert_eq!(contents(&source), contents(&dest), "{name}");
    }
}

#[test]
fn split_parts_come_back_identical() {
    let dir = scratch("split");
    let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
    game(&source);
    let mut job = options(&source, &out, Level::Frost);
    job.split = Some(100_000);
    let report = freeze(&job, &Progress::new()).unwrap();
    assert!(report.outputs.len() > 3);
    let last = report.outputs.last().unwrap();
    let (manifest, _) = open_repack(last).unwrap();
    install(&manifest, last, &dest, vec![true]).unwrap();
    assert_eq!(contents(&source), contents(&dest));
}

#[test]
fn an_optional_part_can_be_left_out() {
    let dir = scratch("optional");
    let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
    game(&source);
    let mut job = options(&source, &out, Level::Frost);
    job.components.push(ComponentRule {
        id: "french".into(),
        name: "French".into(),
        selected: true,
        patterns: vec!["lang/french/**".into()],
        ..Default::default()
    });
    freeze(&job, &Progress::new()).unwrap();
    let repack = out.join("Test.kryo");
    let (manifest, _) = open_repack(&repack).unwrap();

    // The part was never downloaded: the main game must not need it.
    fs::remove_file(out.join("Test.french.kryo")).unwrap();
    let selected = vec![true, false];
    assert!(missing_volumes(&manifest, &Source::new(&repack), &selected).is_empty());
    install(&manifest, &repack, &dest, selected).unwrap();
    assert!(!dest.join("lang").exists());
    assert_eq!(fs::read(dest.join("data/second.bin")).unwrap(), fs::read(source.join("data/second.bin")).unwrap());
}

#[test]
fn an_optional_part_can_lean_on_the_main_game() {
    let dir = scratch("optional-repeat");
    let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
    game(&source);
    let mut job = options(&source, &out, Level::Frost);
    job.components.push(ComponentRule {
        id: "french".into(),
        name: "French".into(),
        selected: true,
        patterns: vec!["lang/french/**".into()],
        ..Default::default()
    });
    freeze(&job, &Progress::new()).unwrap();
    let repack = out.join("Test.kryo");
    let (manifest, _) = open_repack(&repack).unwrap();
    let copy = manifest.files.iter().find(|f| f.path.ends_with("voice-copy.bin")).unwrap();
    assert!(copy.copy_of.is_some());
    install(&manifest, &repack, &dest, vec![true, true]).unwrap();
    assert_eq!(contents(&source), contents(&dest));
}

#[test]
fn a_stopped_install_carries_on() {
    let dir = scratch("resume");
    let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
    game(&source);
    freeze(&options(&source, &out, Level::Frost), &Progress::new()).unwrap();
    let repack = out.join("Test.kryo");
    let (manifest, _) = open_repack(&repack).unwrap();

    // Stopped before any block ran: the second run has to finish from that state.
    let stopped = Progress::new();
    stopped.cancel();
    let job = ThawOptions {
        dest: dest.clone(),
        source: Source::new(&repack),
        selected: vec![true],
        threads: 1,
        memory: 1 << 30,
        verify: true,
    };
    assert!(thaw(&manifest, &job, &stopped).is_err());
    assert!(kryo::thaw::can_resume(&dest, &manifest) || dest.join(".kryo-thaw").exists());
    thaw(&manifest, &job, &Progress::new()).unwrap();
    assert!(!dest.join(".kryo-thaw").exists());
    assert_eq!(contents(&source), contents(&dest));
}

#[test]
fn a_stopped_freeze_keeps_finished_blocks() {
    let dir = scratch("freeze-resume");
    let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
    game(&source);
    let job = options(&source, &out, Level::Frost);
    let stopped = Progress::new();
    stopped.cancel();
    assert!(freeze(&job, &stopped).is_err());
    freeze(&job, &Progress::new()).unwrap();
    let repack = out.join("Test.kryo");
    let (manifest, _) = open_repack(&repack).unwrap();
    install(&manifest, &repack, &dest, vec![true]).unwrap();
    assert_eq!(contents(&source), contents(&dest));
    assert!(!fs::read_dir(&out).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".permafrost")));
}

#[test]
fn damaged_data_is_caught_and_named() {
    let dir = scratch("damage");
    let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
    game(&source);
    freeze(&options(&source, &out, Level::Frost), &Progress::new()).unwrap();
    let repack = out.join("Test.kryo");
    let (manifest, _) = open_repack(&repack).unwrap();
    let mut bytes = fs::read(&repack).unwrap();
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0xff;
    fs::write(&repack, bytes).unwrap();

    let source_of = Source::new(&repack);
    let error = check_data(&manifest, &source_of, &[true], &Progress::new()).unwrap_err();
    assert!(error.to_string().contains("Test.kryo"), "{error}");
    let error = install(&manifest, &repack, &dest, vec![true]).unwrap_err();
    assert!(format!("{error:#}").contains("damaged"), "{error:#}");
}

#[test]
fn a_cut_off_part_is_reported_missing() {
    let dir = scratch("short");
    let (source, out) = (dir.join("src"), dir.join("out"));
    game(&source);
    freeze(&options(&source, &out, Level::Frost), &Progress::new()).unwrap();
    let repack = out.join("Test.kryo");
    let (manifest, _) = open_repack(&repack).unwrap();
    let file = fs::OpenOptions::new().write(true).open(&repack).unwrap();
    file.set_len(1000).unwrap();
    assert_eq!(missing_volumes(&manifest, &Source::new(&repack), &[true]), vec!["Test.kryo"]);
}

#[test]
fn excluded_files_stay_out() {
    let dir = scratch("exclude");
    let (source, out) = (dir.join("src"), dir.join("out"));
    game(&source);
    let mut job = options(&source, &out, Level::Frost);
    job.exclude.push("**/*.mp4".into());
    freeze(&job, &Progress::new()).unwrap();
    let (manifest, _) = open_repack(&out.join("Test.kryo")).unwrap();
    assert!(!manifest.files.iter().any(|f| f.path.ends_with(".mp4")));
}

#[test]
fn the_output_cannot_sit_inside_the_game() {
    let dir = scratch("inside");
    let source = dir.join("src");
    game(&source);
    let job = options(&source, &source.join("out"), Level::Frost);
    assert!(freeze(&job, &Progress::new()).is_err());
}

#[test]
fn paths_cannot_climb_out_of_the_install_folder() {
    let root = Path::new("install");
    for path in ["../evil.dll", "a/../../evil.dll", "/etc/passwd", "C:/Windows/evil.dll", r"..\evil.dll"] {
        let joined = safe_join(root, path);
        if cfg!(windows) || !path.contains('\\') {
            assert!(joined.is_err(), "{path} got through");
        }
    }
    assert!(safe_join(root, "data/ok.bin").is_ok());
}

#[test]
fn a_repack_with_a_setup_reads_back_from_the_setup() {
    let dir = scratch("embed");
    let (source, out, dest) = (dir.join("src"), dir.join("out"), dir.join("dest"));
    game(&source);
    let mut job = options(&source, &out, Level::Frost);
    job.stub = Some(b"MZ pretend program".to_vec());
    job.embed = Embed::Auto;
    let report = freeze(&job, &Progress::new()).unwrap();
    let setup = report.setup.unwrap();
    assert!(setup.to_string_lossy().ends_with("Test Setup.exe"));
    let (manifest, _) = open_repack(&setup).unwrap();
    assert!(manifest.components[0].volumes[0].name.is_empty());
    install(&manifest, &setup, &dest, vec![true]).unwrap();
    assert_eq!(contents(&source), contents(&dest));
}
