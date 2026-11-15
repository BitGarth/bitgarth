mod args;
mod client;
mod commands;
mod context;
mod model;
mod profiles;
mod ui;

use clap::Parser as _;

use context::{CliError, Context};

fn main() {
    let args = args::Args::parse();
    let context = Context::new(args.global.clone());
    let exit_code = match commands::run(&context, args.command) {
        Ok(()) => 0,
        Err(error) => report(&error),
    };
    std::process::exit(exit_code);
}

fn report(error: &CliError) -> i32 {
    eprintln!(
        "{} {}",
        ui::epaint(ui::Style::Red, "✗"),
        ui::epaint(ui::Style::Bold, &error.to_string())
    );
    for detail in error.details() {
        eprintln!("  {}", ui::epaint(ui::Style::Red, &format!("• {detail}")));
    }
    if let Some(hint) = error.hint() {
        ui::hint(&hint);
    }
    error.exit_code()
}
