#[allow(dead_code)]
mod cli;
#[allow(dead_code)]
mod profile;

fn main() {
    let result = match cli::parse_args(std::env::args_os().skip(1)) {
        Ok(cli::Parsed::Help { command }) => {
            print!("{}", cli::help::render(command.as_deref()));
            return;
        }
        Ok(cli::Parsed::Schema { topic }) => {
            println!(
                "{}",
                cli::schema_topic(topic.as_deref()).expect("validated schema topic")
            );
            return;
        }
        Ok(cli::Parsed::Command { request, start }) => cli::execute(&request, start)
            .unwrap_or_else(|error| cli::Response::failure("connection_failed", error)),
        Err(error) => cli::Response::failure(
            "invalid_arguments",
            format!("{error}. No request was sent."),
        ),
    };
    println!("{}", serde_json::to_string(&result).expect("JSON response"));
    if !result.ok {
        std::process::exit(1);
    }
    if result.result.get("complete") == Some(&serde_json::Value::Bool(false)) {
        std::process::exit(2);
    }
}
