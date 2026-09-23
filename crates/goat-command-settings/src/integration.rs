use goat_client::AdminRequest;
use goat_command::{Command, CommandEffect, CommandInvocation, Session};
use goat_protocol::NotifyKind;

pub struct Integration;

impl Command for Integration {
    fn name(&self) -> &'static str {
        "integration"
    }

    fn description(&self) -> &'static str {
        "show integration connections (Linear, Sentry, …) and check them"
    }

    fn run(&self, invocation: CommandInvocation, session: &mut dyn Session) -> CommandEffect {
        match request(invocation.raw_args.trim()) {
            Ok(request) => CommandEffect::Admin(vec![request]),
            Err(message) => {
                session.notify(NotifyKind::Error, message);
                CommandEffect::Noop
            }
        }
    }
}

fn request(args: &str) -> Result<AdminRequest, String> {
    match args {
        "" | "list" => Ok(AdminRequest::IntegrationStatus { verify: false }),
        "verify" => Ok(AdminRequest::IntegrationStatus { verify: true }),
        other => Err(format!(
            "unknown /integration subcommand: {other} (try list or verify; \
             connect new services with `goat integration add` in a terminal)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_and_list_show_status_and_verify_checks_it() {
        assert!(matches!(
            request(""),
            Ok(AdminRequest::IntegrationStatus { verify: false })
        ));
        assert!(matches!(
            request("list"),
            Ok(AdminRequest::IntegrationStatus { verify: false })
        ));
        assert!(matches!(
            request("verify"),
            Ok(AdminRequest::IntegrationStatus { verify: true })
        ));
        assert!(request("add linear").is_err());
    }
}
