
// Copyright 2020 The Evcxr Authors.
//
// Licensed under the Apache License, Version 2.0 <LICENSE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE
// or https://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.

use anyhow::Result;
use ariadne::sources;
use colored::*;
use evcxr::CommandContext;
use evcxr::CompilationError;
use evcxr::Error;
use evcxr::Theme;
use evcxr_repl::BgInitMutex;
use evcxr_repl::EvcxrRustylineHelper;
use rustyline::error::ReadlineError;
use rustyline::At;
use rustyline::Cmd;
use rustyline::EditMode;
use rustyline::Editor;
use rustyline::ExternalPrinter;
use rustyline::KeyCode;
use rustyline::KeyEvent;
use rustyline::Modifiers;
use rustyline::Movement;
use rustyline::Word;
use std::fs;
use std::io;
use std::sync::Arc;
use structopt::StructOpt;

const PROMPT: &str = ">> ";

struct Repl {
    command_context: Arc<BgInitMutex<Result<CommandContext, Error>>>,
    ide_mode: bool,
}

fn send_output<T: io::Write + Send + 'static>(
    channel: crossbeam_channel::Receiver<String>,
    mut printer: Option<impl ExternalPrinter + Send + 'static>,
    mut fallback_output: T,
    color: Option<Color>,
) {
    std::thread::spawn(move || {
        while let Ok(line) = channel.recv() {
            let to_print = if let Some(color) = color {
                format!("{}\n", line.color(color))
            } else {
                format!("{line}\n")
            };
            if let Some(printer) = printer.as_mut() {
                if printer.print(to_print).is_err() {
                    break;
                }
            } else if write!(fallback_output, "{to_print}").is_err() {
                break;
            }
        }
    });
}

impl Repl {
    fn new(ide_mode: bool, opt: String, editor: &mut Editor<EvcxrRustylineHelper>) -> Repl {
        let stdout_printer = editor.create_external_printer().ok();
        let stderr_printer = editor.create_external_printer().ok();
        let stderr_colour = Some(Color::BrightRed);
        let initialize = move || -> Result<CommandContext, Error> {
            let (mut command_context, outputs) = CommandContext::new()?;

            send_output(outputs.stdout, stdout_printer, io::stdout(), None);
            send_output(outputs.stderr, stderr_printer, io::stderr(), stderr_colour);
            command_context.execute(":load_config --quiet")?;
            if !opt.is_empty() {
                // Ignore failure
                command_context.set_opt_level(&opt).ok();
            }
            setup_ctrlc_handler(&command_context);
            Ok(command_context)
        };
        let command_context = Arc::new(BgInitMutex::new(initialize));
        Repl {
            command_context,
            ide_mode,
        }
    }
    fn execute(&mut self, to_run: &str) -> Result<(), Error> {
        let execution_result = match &mut *self.command_context.lock() {
            Ok(context) => context.execute(to_run),
            Err(error) => return Err(error.clone()),
        };
        let success = match execution_result {
            Ok(output) => {
                if let Some(text) = output.get("text/plain") {
                    println!("{text}");
                }
                if let Some(duration) = output.timing {
                    println!("{}", format!("Took {}ms", duration.as_millis()).blue());

                    for phase in output.phases {
                        println!(
                            "{}",
                            format!("  {}: {}ms", phase.name, phase.duration.as_millis()).blue()
                        );
                    }
                }
                true
            }
            Err(evcxr::Error::CompilationErrors(errors)) => {
                self.display_errors(to_run, errors);
                false
            }
            Err(err) => {
                eprintln!("{}", format!("{err}").bright_red());
                false
            }
        };

        if self.ide_mode {
            let success_marker = if success { "\u{0091}" } else { "\u{0092}" };
            print!("{success_marker}");
        }
        Ok(())
    }

    fn display_errors(&mut self, source: &str, errors: Vec<CompilationError>) {
        use yansi::Paint;
        if cfg!(windows) && !Paint::enable_windows_ascii() {
            Paint::disable()
        }
        let mut last_span_lines: &Vec<String> = &vec![];
        for error in &errors {
            if error.is_from_user_code() {
                if let Some(report) =
                    error.build_report("command".to_string(), source.to_string(), Theme::Dark)
                {