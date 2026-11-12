//! `tokens …`: the class-token admin command, the same tool as `web-server tokens` of the Kotlin server.
//!
//! Run it as the service user; the running server picks up changes within about a second.

fn main() {
    std::process::exit(xiaoman_web::tokens_admin::run_from_env());
}
