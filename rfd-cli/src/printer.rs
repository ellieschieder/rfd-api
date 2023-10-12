use rfd_sdk::types::{Error, ListRfd};
use std::{fs::File, io::Write, process::Command};
use tabwriter::TabWriter;

use crate::generated::cli::CliOutput;

static HEADER_COLOR: &str = "\x1b[38;5;245m";
static TEXT_COLOR: &str = "253";
static ERROR_COLOR: &str = "\x1b[38;2;251;110;136m";
static RESET_COLOR: &str = "\x1b[0m";
pub struct RfdCliPrinter {}

impl CliOutput for RfdCliPrinter {
    fn output_get_rfd(
        &self,
        response: Result<rfd_sdk::types::FullRfd, progenitor_client::Error<Error>>,
    ) {
        match response {
            Ok(rfd) => {
                let mut tw = TabWriter::new(vec![]).ansi(true);

                writeln!(
                    &mut tw,
                    "{}Number:\t\x1b[38;5;{}m{}",
                    HEADER_COLOR, TEXT_COLOR, rfd.rfd_number,
                );
                writeln!(
                    &mut tw,
                    "{}Title:\t\x1b[38;5;{}m{}",
                    HEADER_COLOR, TEXT_COLOR, rfd.title,
                );
                writeln!(
                    &mut tw,
                    "{}State:\t{}{}",
                    HEADER_COLOR,
                    state_color(&rfd.state),
                    rfd.state.unwrap_or_default(),
                );
                writeln!(
                    &mut tw,
                    "{}Authors:\t\x1b[38;5;{}m{}",
                    HEADER_COLOR,
                    TEXT_COLOR,
                    rfd.authors.unwrap_or_default(),
                );
                writeln!(
                    &mut tw,
                    "{}Latest Commit:\t\x1b[38;5;{}m{}",
                    HEADER_COLOR, TEXT_COLOR, rfd.commit,
                );
                writeln!(
                    &mut tw,
                    "{}Updated At:\t\x1b[38;5;{}m{}",
                    HEADER_COLOR, TEXT_COLOR, rfd.committed_at,
                );
                writeln!(
                    &mut tw,
                    "{}Url:\t\x1b[38;5;{}mhttps://rfd.shared.oxide.computer/rfd/{}",
                    HEADER_COLOR, TEXT_COLOR, rfd.rfd_number,
                );
                writeln!(
                    &mut tw,
                    "{}Discussion Url:\t\x1b[38;5;{}m{}",
                    HEADER_COLOR,
                    TEXT_COLOR,
                    rfd.discussion.unwrap_or_default(),
                );
                writeln!(&mut tw, "{}---------------", HEADER_COLOR,);
                writeln!(&mut tw, "\x1b[38;5;{}m", TEXT_COLOR);

                let body = print_rfd_html(&rfd.content);

                if let Some((header, body)) = body.split_once("State") {
                    for line in textwrap::wrap(body.trim_start(), 200).iter().skip(1) {
                        writeln!(&mut tw, "{}", line);
                    }

                    writeln!(&mut tw, "{}---------------", HEADER_COLOR,);
                    writeln!(&mut tw, "");
                    writeln!(
                        &mut tw,
                        "{}...someday the content will be nicely formatted once I can render AsciiDoc to a terminal...",
                        HEADER_COLOR,
                    );
                }

                let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
                println!("{}", written);
            }
            Err(err) => print_error(err),
        }
    }

    fn output_get_rfds(
        &self,
        mut response: Result<Vec<rfd_sdk::types::ListRfd>, progenitor_client::Error<Error>>,
    ) {
        match response {
            Ok(mut response) => print_rfd_list(&mut response),
            Err(err) => print_error(err),
        }
    }

    fn output_search_rfds(
        &self,
        response: Result<Vec<rfd_sdk::types::ListRfd>, progenitor_client::Error<Error>>,
    ) {
        match response {
            Ok(mut response) => print_rfd_list(&mut response),
            Err(err) => print_error(err),
        }
    }

    fn output_get_self(
        &self,
        response: Result<rfd_sdk::types::ApiUserForApiPermission, progenitor_client::Error<Error>>,
    ) {
    }

    fn output_get_api_user(
        &self,
        response: Result<rfd_sdk::types::ApiUserForApiPermission, progenitor_client::Error<Error>>,
    ) {
    }
}

fn print_error(error: progenitor_client::Error<Error>) {
    let mut tw = TabWriter::new(vec![]).ansi(true);

    match error {
        progenitor_client::Error::CommunicationError(err) => {
            writeln!(
                &mut tw,
                "{}Failed to reach the API server{}",
                ERROR_COLOR, RESET_COLOR
            );
            writeln!(&mut tw, "{:#?}", err);
        }
        progenitor_client::Error::ErrorResponse(err) => {
            writeln!(
                &mut tw,
                "{}Received error from the remote API{}",
                ERROR_COLOR, RESET_COLOR
            );
            writeln!(
                &mut tw,
                "{}Message\t{}{}",
                HEADER_COLOR, RESET_COLOR, err.message
            );
            if let Some(code) = &err.error_code {
                writeln!(&mut tw, "{}Code\t{}{}", HEADER_COLOR, RESET_COLOR, code);
            }
            writeln!(
                &mut tw,
                "{}Request\t{}{}",
                HEADER_COLOR, RESET_COLOR, err.request_id
            );
        }
        progenitor_client::Error::InvalidRequest(err) => {
            writeln!(&mut tw, "{}Internal CLI error{}", ERROR_COLOR, RESET_COLOR);
            writeln!(&mut tw, "{:?}", err);
            writeln!(
                &mut tw,
                "{}Please report this as a bug against the rfd-api{}",
                HEADER_COLOR, RESET_COLOR
            );
        }
        progenitor_client::Error::InvalidResponsePayload(err) => {
            writeln!(&mut tw, "{}Internal CLI error{}", ERROR_COLOR, RESET_COLOR);
            writeln!(&mut tw, "{:?}", err);
            writeln!(
                &mut tw,
                "{}Please report this as a bug against the rfd-api{}",
                HEADER_COLOR, RESET_COLOR
            );
        }
        progenitor_client::Error::UnexpectedResponse(err) => {
            writeln!(&mut tw, "{}Internal CLI error{}", ERROR_COLOR, RESET_COLOR);
            writeln!(&mut tw, "{:?}", err);
            writeln!(
                &mut tw,
                "{}Please report this as a bug against the rfd-api{}",
                HEADER_COLOR, RESET_COLOR
            );
        }
    }
    tw.flush().unwrap();

    let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
    println!("{}", written);
}

fn print_rfd_html(content: &str) -> String {
    let mut tmp_content = File::create("adoc-source.adoc").unwrap();
    tmp_content.write_all(content.as_bytes());

    let html = Command::new("asciidoctor")
        .arg("adoc-source.adoc")
        .output()
        .unwrap()
        .stdout;

    let text = String::from_utf8(
        Command::new("w3m")
            .arg("-dump")
            .arg("adoc-source.html")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();

    std::fs::remove_file("adoc-source.adoc").unwrap();
    std::fs::remove_file("adoc-source.html").unwrap();

    text
}

fn print_rfd_list(rfds: &mut [ListRfd]) {
    let mut tw = TabWriter::new(vec![]).ansi(true);

    writeln!(
        &mut tw,
        "\x1b[38;5;{}mNumber\tTitle\tState\tLatest Commit\tUpdated At",
        HEADER_COLOR
    );
    writeln!(
        &mut tw,
        "\x1b[38;5;{}m------\t-----\t-----\t-------------\t----------",
        HEADER_COLOR
    );

    for mut rfd in rfds.iter_mut() {
        rfd.title.truncate(90);

        writeln!(
            &mut tw,
            "\x1b[38;5;{}m{}\t{}\t{}{}\t\x1b[38;5;{}m{}",
            TEXT_COLOR,
            rfd.rfd_number,
            rfd.title,
            state_color(&rfd.state),
            rfd.state.as_deref().unwrap_or_default(),
            TEXT_COLOR,
            // rfd.sha,
            rfd.committed_at
        );
    }
    tw.flush().unwrap();

    let written = String::from_utf8(tw.into_inner().unwrap()).unwrap();
    println!("{}", written);
}

fn state_color(state: &Option<String>) -> &'static str {
    match state.as_deref() {
        Some("published") => "\x1b[38;2;72;213;151m",
        Some("committed") => "\x1b[38;2;72;213;151m",
        Some("discussion") => "\x1b[38;2;139;161;255m",
        Some("prediscussion") => "\x1b[38;2;163;128;203m",
        Some("ideation") => "\x1b[38;2;245;185;68m",
        Some("abandoned") => "\x1b[38;2;231;231;232m",
        _ => "\x1b[38;2;231;231;232m",
    }
}
