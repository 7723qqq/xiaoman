//! `ios-runner`: operate an iPhone through WebDriverAgent.
//!
//! All commands live in [`xiaoman_ios::runner`]; this binary only wires up Ctrl-C and the exit code.

use std::sync::Arc;

use xiaoman_ios::runner::command::{CtrlCStop, Runner, install_ctrl_c};

#[tokio::main]
async fn main() {
    let stop = Arc::new(CtrlCStop::new());
    install_ctrl_c(stop.clone());
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = Runner::new().with_stop(stop).run(args).await;
    std::process::exit(code);
}
