use argparse::{ArgumentParser, Store, StoreOption, Collect};
use std::path::PathBuf;

pub struct CliArgs {
    pub input_files: Vec<PathBuf>,
    pub freqs: Vec<String>,
    pub rtypes: Vec<String>,
    pub rids: Vec<String>,
    pub tgs: Vec<String>,
    pub nacs: Vec<String>,
    pub tz: Option<String>,
    pub record_dir: Option<PathBuf>,
    pub transcriber: String,
    pub log_level: String,
    pub out: PathBuf,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            input_files: vec![],
            freqs: vec![],
            rtypes: vec![],
            rids: vec![],
            tgs: vec![],
            nacs: vec![],
            tz: None,
            record_dir: None,
            transcriber: "none".into(),
            log_level: "essential".into(),
            out: std::path::PathBuf::from("out.csv"),
        }
    }
}

pub fn parse_cli() -> CliArgs {
    let mut args = CliArgs::default();
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("DSDPlus SRT -> CSV converter");
        ap.refer(&mut args.input_files)
            .add_argument("input_files", Collect, "Input SRT files (one or more)");
        ap.refer(&mut args.freqs)
            .add_option(&["-f", "--freq"], Collect, "Filter by frequency");
        ap.refer(&mut args.rtypes)
            .add_option(&["-t", "--type"], Collect, "Filter by radio type");
        ap.refer(&mut args.rids)
            .add_option(&["--rid"], Collect, "Filter by radio ID");
        ap.refer(&mut args.tgs)
            .add_option(&["--tg"], Collect, "Filter by talk group");
        ap.refer(&mut args.nacs)
            .add_option(&["--nac"], Collect, "Filter by NAC");
        ap.refer(&mut args.tz)
            .add_option(&["--tz"], StoreOption, "Timezone (IANA name)");
        ap.refer(&mut args.record_dir)
            .add_option(&["--record-dir"], StoreOption, "Record directory (with YYYYMMDD subfolders)");
        ap.refer(&mut args.transcriber)
            .add_option(&["--transcriber"], Store, "Transcriber: none|text");
        ap.refer(&mut args.log_level)
            .add_option(&["--log"], Store, "Log level (essential|debug|trace|warn|error)");
        ap.refer(&mut args.out)
            .add_option(&["--out"], Store, "Output CSV path");
        ap.parse_args_or_exit();
    }
    args
}
