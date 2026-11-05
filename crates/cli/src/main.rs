//! `permafrost`: everything the app does, for scripts and batch runs.

use anyhow::{bail, Context, Result};
use kryo::format::{open_repack, Assets, Game};
use kryo::freeze::{freeze, safe_file_name, Embed, Level, Options};
use kryo::progress::Progress;
use kryo::scan::inspect;
use kryo::thaw::{check_data, thaw, Source, ThawOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

const HELP: &str = "\
Kryoto Permafrost

  permafrost freeze <folder> --out <folder> [options]
      --level frost|deep|zero   how hard to work (default deep)
      --title <text>            game title (default: from the folder name)
      --exe <path>              program to start, relative to the folder
      --by <text>               who made the repack
      --memory <GiB>            memory for compressing (default: half of this PC's)
      --threads <n>             threads (default: all)
      --split <MiB>             largest .kryo part
      --stub <file>             setup program to build a setup with
      --beside                  keep the data beside the setup, even if it fits inside
      --exclude <pattern>       leave files out (repeat for more)
      --no-test                 skip the byte-for-byte check at the end
  permafrost thaw <repack> <folder>     install a setup or .kryo
  permafrost check <repack>             check a repack's data
  permafrost list <repack>              show what a repack holds
  permafrost inspect <folder>           show what the app would suggest
";

fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

fn watch<T: Send>(progress: Arc<Progress>, work: impl FnOnce() -> Result<T> + Send) -> Result<T> {
    std::thread::scope(|scope| {
        let handle = scope.spawn(work);
        let mut stage = "";
        while !handle.is_finished() {
            std::thread::sleep(Duration::from_millis(500));
            let now = progress.stage().id();
            if now != stage {
                if !stage.is_empty() {
                    eprintln!();
                }
                stage = now;
            }
            let done = progress.done.load(Ordering::Relaxed);
            let total = progress.total.load(Ordering::Relaxed).max(1);
            let packed = progress.packed.load(Ordering::Relaxed);
            eprint!("\r{stage:<12} {:>5.1}%  {} of {}  packed {}      ", done as f64 * 100.0 / total as f64, size(done), size(total), size(packed));
        }
        eprintln!();
        handle.join().unwrap()
    })
}

fn total_memory() -> u64 {
    // A safe guess where the OS gives no answer without pulling in a dependency.
    std::env::var("PERMAFROST_MEMORY_GIB").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(8) << 30
}

fn run_freeze(mut args: std::vec::IntoIter<String>) -> Result<()> {
    let source = PathBuf::from(args.next().context("which folder?")?);
    let mut out = None;
    let mut options = Options {
        name: String::new(),
        game: Game::default(),
        level: Level::DeepFreeze,
        memory: total_memory(),
        threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        split: None,
        embed: Embed::Auto,
        exclude: Vec::new(),
        components: Vec::new(),
        redists: Vec::new(),
        assets: Assets { tune: true, accent: [125, 211, 252], ..Default::default() },
        stub: None,
        test: true,
        source: source.clone(),
        out_dir: PathBuf::new(),
    };
    let mut title = None;
    let mut exe = None;
    while let Some(flag) = args.next() {
        let mut value = || args.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(value()?)),
            "--level" => {
                options.level = match value()?.as_str() {
                    "frost" => Level::Frost,
                    "deep" => Level::DeepFreeze,
                    "zero" => Level::AbsoluteZero,
                    other => bail!("unknown level {other}"),
                }
            }
            "--title" => title = Some(value()?),
            "--exe" => exe = Some(value()?),
            "--by" => options.game.repacker = value()?,
            "--memory" => options.memory = value()?.parse::<u64>()? << 30,
            "--threads" => options.threads = value()?.parse()?,
            "--split" => options.split = Some(value()?.parse::<u64>()? << 20),
            "--stub" => options.stub = Some(std::fs::read(value()?).context("reading the stub")?),
            "--exclude" => options.exclude.push(value()?),
            "--beside" => options.embed = Embed::Never,
            "--no-test" => options.test = false,
            other => bail!("unknown option {other}"),
        }
    }
    let found = inspect(&source)?;
    options.out_dir = out.context("--out is required")?;
    options.game.title = title.unwrap_or(found.title);
    options.game.exe = exe.unwrap_or(found.exe);
    options.name = safe_file_name(&options.game.title);
    options.components = found.components;
    options.redists = found.redists.into_iter().map(Into::into).collect();

    let progress = Arc::new(Progress::new());
    let report = {
        let progress2 = progress.clone();
        let options = &options;
        watch(progress, move || freeze(options, &progress2))?
    };
    println!();
    println!("{} -> {} ({:.1}%) in {:.0}s", size(report.original), size(report.total_output), report.total_output as f64 * 100.0 / report.original.max(1) as f64, report.seconds);
    for class in &report.classes {
        println!("  {:<18} {:>6} files  {:>10} -> {:>10}  {:>5.1}%", class.label, class.files, size(class.original), size(class.packed), class.packed as f64 * 100.0 / class.original.max(1) as f64);
    }
    println!("repeated data   {}", size(report.duplicate_bytes));
    println!("streams opened  {} ({})", report.unwrapped_streams, size(report.unwrapped_bytes));
    println!("jpegs           {} (saved {})", report.jpegs, size(report.jpeg_saved));
    println!("install memory  {}", size(report.memory_to_install));
    for codec in &report.codecs {
        println!("  {:<12} {:>3} blocks  {} -> {}", codec.codec, codec.blocks, size(codec.stored), size(codec.packed));
    }
    for path in &report.outputs {
        println!("{}", path.display());
    }
    Ok(())
}

fn run_thaw(repack: &Path, dest: &Path) -> Result<()> {
    let (manifest, _) = open_repack(repack)?;
    let options = ThawOptions {
        dest: dest.to_path_buf(),
        source: Source::new(repack),
        selected: manifest.components.iter().map(|c| c.selected).collect(),
        threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        memory: total_memory(),
        verify: true,
    };
    let progress = Arc::new(Progress::new());
    let started = std::time::Instant::now();
    {
        let progress2 = progress.clone();
        let (manifest, options) = (&manifest, &options);
        watch(progress, move || thaw(manifest, options, &progress2))?;
    }
    println!("Installed {} in {:.0}s.", manifest.game.title, started.elapsed().as_secs_f64());
    Ok(())
}

fn run_check(repack: &Path) -> Result<()> {
    let (manifest, _) = open_repack(repack)?;
    let selected = vec![true; manifest.components.len()];
    let progress = Progress::new();
    check_data(&manifest, &Source::new(repack), &selected, &progress)?;
    println!("All {} blocks are good.", manifest.blocks.len());
    Ok(())
}

fn run_list(repack: &Path) -> Result<()> {
    let (manifest, assets) = open_repack(repack)?;
    let game = &manifest.game;
    println!("{} {}", game.title, game.version);
    println!("starts {}", game.exe);
    println!("{} files, {} -> {}", manifest.stats.files, size(manifest.stats.original), size(manifest.stats.packed));
    println!("art: {}", if assets.cover.is_some() { "yes" } else { "no" });
    for component in &manifest.components {
        println!("{:<24} {:>10} installed  {:>10} packed  {} part(s)", component.name, size(component.size), size(component.packed), component.volumes.len());
    }
    for block in &manifest.blocks {
        println!("  block {:<12?} {} -> {}  {} runs", block.codec.label(), size(block.stored), size(block.packed), block.entries.len());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("freeze") => run_freeze(args),
        Some("thaw") => {
            let repack = PathBuf::from(args.next().context("which repack?")?);
            let dest = PathBuf::from(args.next().context("which folder?")?);
            run_thaw(&repack, &dest)
        }
        Some("check") => run_check(Path::new(&args.next().context("which repack?")?)),
        Some("list") => run_list(Path::new(&args.next().context("which repack?")?)),
        Some("inspect") => {
            let found = inspect(Path::new(&args.next().context("which folder?")?))?;
            println!("{found:#?}");
            Ok(())
        }
        _ => {
            print!("{HELP}");
            Ok(())
        }
    }
}
