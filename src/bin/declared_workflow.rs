//! Unpublished declared-workflow entry point over the private ordinary-Cargo
//! route.
//!
//! `plan <declaration>` resolves one versioned project/workflow declaration
//! into a read-only effective plan with no side effects. `run <declaration>
//! <bootstrap>` binds that declaration to one protected owner-issued attempt
//! before any acquire and then delegates to the unchanged private route.

use oxide_storage::ordinary_cargo::{
    run_declared_attempt, DeclaredWorkflowError, OrdinaryCargoAttemptResult,
};
use oxide_storage::{read_workflow_declaration, WorkflowPlan};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(command) = args.next() else {
        return ExitCode::from(2);
    };
    if command == "plan" {
        let Some(declaration) = args.next() else {
            return ExitCode::from(2);
        };
        if args.next().is_some() {
            return ExitCode::from(2);
        }
        let bytes = match read_workflow_declaration(Path::new(&declaration)) {
            Ok(bytes) => bytes,
            Err(refusal) => {
                eprintln!("{}", refusal.render());
                return ExitCode::from(1);
            }
        };
        return match WorkflowPlan::resolve(&bytes) {
            Ok(plan) => {
                print!("{}", plan.render());
                ExitCode::SUCCESS
            }
            Err(refusal) => {
                eprintln!("{}", refusal.render());
                ExitCode::from(1)
            }
        };
    }
    if command == "run" {
        let Some(declaration) = args.next() else {
            return ExitCode::from(2);
        };
        let Some(bootstrap) = args.next() else {
            return ExitCode::from(2);
        };
        if args.next().is_some() {
            return ExitCode::from(2);
        }
        let bytes = match read_workflow_declaration(Path::new(&declaration)) {
            Ok(bytes) => bytes,
            Err(refusal) => {
                eprintln!("{}", refusal.render());
                return ExitCode::from(1);
            }
        };
        return match run_declared_attempt(Path::new(&bootstrap), &bytes) {
            Ok(OrdinaryCargoAttemptResult::Finalized(_)) => ExitCode::SUCCESS,
            Ok(OrdinaryCargoAttemptResult::Refused) => {
                eprintln!("declared workflow refused before host submission");
                ExitCode::from(1)
            }
            Ok(OrdinaryCargoAttemptResult::Held(phase)) => {
                eprintln!("declared workflow ownership is held pending recovery ({phase:?})");
                ExitCode::from(2)
            }
            Err(DeclaredWorkflowError::Refusal(refusal)) => {
                eprintln!("{}", refusal.render());
                ExitCode::from(1)
            }
            Err(DeclaredWorkflowError::Attempt(_)) => {
                eprintln!("declared workflow attempt process refused or timed out");
                ExitCode::from(3)
            }
        };
    }
    ExitCode::from(2)
}
