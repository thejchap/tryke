mod clean;
mod graph;
mod server;
mod test;

pub(crate) use clean::run_clean_command;
pub(crate) use graph::run_graph_command;
pub(crate) use server::run_server_command;
pub(crate) use test::run_test_command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandOrigin {
    Bare,
    Explicit,
}
