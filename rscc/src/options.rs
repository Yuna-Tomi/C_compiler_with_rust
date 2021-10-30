use clap::Clap;

#[derive(Clap, Debug)]
#[clap(
    name = "rscc",
    version = "0.0.0",
    author = "Yuna Tomida",
    about = "学習のためにCコンパイラをRustで自作します。"
)]

pub struct Opts {
    // // verbose level
    // #[clap(short, long)]
    // verbose: bool,

    // 入力ファイル名
    #[clap(name = "FILE")]
    pub input_file: Option<String>,
}