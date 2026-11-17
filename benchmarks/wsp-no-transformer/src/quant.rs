//! 三值化模式。

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quant {
    None,
    Sign,
    SignRow,
    Threshold,
}

impl Quant {
    pub fn from_env_var(name: &str) -> Self {
        match std::env::var(name).unwrap_or_default().as_str() {
            "" | "none" => Quant::None,
            "sign" => Quant::Sign,
            "sign_row" => Quant::SignRow,
            "threshold" => Quant::Threshold,
            other => {
                eprintln!("{name} 只支持 none/sign/sign_row/threshold，收到 {other}");
                std::process::exit(2);
            }
        }
    }
}
