use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Test,
    Discover,
}

fn run_test() -> &'static str {
    "test"
}

fn run_discover() -> &'static str {
    "discover"
}

fn main() {
    let cli = Cli::parse();
    match &cli.command {
        Commands::Test => println!("{}", run_test()),
        Commands::Discover => println!("{}", run_discover()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_command() {
        assert_snapshot!(run_test());
    }

    #[test]
    fn discover_command() {
        assert_snapshot!(run_discover());
    }
}
