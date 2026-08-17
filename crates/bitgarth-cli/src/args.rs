use clap::{CommandFactory, Parser, Subcommand, error::ErrorKind};

#[derive(Parser)]
#[command(name = "bitgarth", version)]
pub(crate) struct Args {
    #[arg(long, global = true, value_parser = parse_profile_name)]
    pub(crate) profile: Vec<String>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    #[command(name = "balancesheet", visible_alias = "bs")]
    BalanceSheet,
    Pair {
        #[arg(value_name = "BITGARTH_URL")]
        bitgarth_url: Option<String>,
        #[arg(long)]
        allow_insecure_http: bool,
    },
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
}

impl Args {
    pub(crate) fn parse() -> Self {
        match Self::try_parse_from(std::env::args_os()) {
            Ok(args) => args,
            Err(error) => error.exit(),
        }
    }

    pub(crate) fn try_parse_from<I, T>(values: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let values: Vec<std::ffi::OsString> = values.into_iter().map(Into::into).collect();
        let profile_count = values
            .iter()
            .filter(|value| {
                let value = value.to_string_lossy();
                value == "--profile" || value.starts_with("--profile=")
            })
            .count();
        if profile_count > 1 {
            return Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "--profile may be supplied only once",
            ));
        }
        <Self as Parser>::try_parse_from(values)
    }

    pub(crate) fn validate(&self) -> Result<(), clap::Error> {
        match &self.command {
            Command::Pair {
                bitgarth_url,
                allow_insecure_http,
            } => bitgarth_url.as_deref().map_or(Ok(()), |input| {
                let origin = crate::profiles::canonicalize_origin(input).map_err(|error| {
                    Self::command().error(ErrorKind::InvalidValue, error.to_string())
                })?;
                if origin.scheme() == "https" && *allow_insecure_http {
                    return Err(Self::command().error(
                        ErrorKind::InvalidValue,
                        "--allow-insecure-http is only valid with an HTTP BitGarth URL",
                    ));
                }
                Ok(())
            }),
            Command::BalanceSheet => Ok(()),
            Command::Profile { .. } if !self.profile.is_empty() => Err(Self::command().error(
                ErrorKind::ArgumentConflict,
                "--profile cannot be used with profile management commands",
            )),
            Command::Profile { .. } => Ok(()),
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum ProfileCommand {
    List,
    Remove {
        #[arg(value_parser = parse_profile_name)]
        name: String,
    },
    Rename {
        #[arg(value_parser = parse_profile_name)]
        old: String,
        #[arg(value_parser = parse_profile_name)]
        new: String,
    },
}

fn parse_profile_name(value: &str) -> Result<String, String> {
    crate::profiles::validate_profile_name(value)
        .map(|()| value.to_owned())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::{Args, Command, ProfileCommand};

    #[test]
    fn parses_only_named_profile_removal() {
        let args = Args::try_parse_from(["bitgarth", "profile", "remove", "personal"]);
        assert!(args.is_ok());
        let Ok(args) = args else {
            return;
        };

        assert!(matches!(
            args.command,
            Command::Profile {
                command: ProfileCommand::Remove { name }
            } if name == "personal"
        ));
    }

    #[test]
    fn parses_profile_list_and_rename() {
        let list = Args::try_parse_from(["bitgarth", "profile", "list"]);
        assert!(list.is_ok_and(|args| matches!(
            args.command,
            Command::Profile {
                command: ProfileCommand::List
            }
        )));

        let rename = Args::try_parse_from(["bitgarth", "profile", "rename", "personal", "primary"]);
        assert!(rename.is_ok_and(|args| matches!(
            args.command,
            Command::Profile {
                command: ProfileCommand::Rename { old, new }
            } if old == "personal" && new == "primary"
        )));
    }

    #[test]
    fn pair_accepts_profile_before_or_after_subcommand() {
        for argv in [
            vec![
                "bitgarth",
                "--profile",
                "personal",
                "pair",
                "https://example.com",
            ],
            vec![
                "bitgarth",
                "pair",
                "--profile",
                "personal",
                "https://example.com",
            ],
        ] {
            let result = Args::try_parse_from(argv).and_then(|args| {
                args.validate()?;
                Ok(args)
            });
            assert!(result.is_ok());
        }
    }

    #[test]
    fn balancesheet_and_bs_are_the_same_command() {
        for argv in [
            vec!["bitgarth", "balancesheet"],
            vec!["bitgarth", "bs"],
            vec!["bitgarth", "--profile", "personal", "balancesheet"],
            vec!["bitgarth", "bs", "--profile", "personal"],
        ] {
            let parsed = Args::try_parse_from(argv);
            assert!(parsed.is_ok());
            let Ok(parsed) = parsed else { continue };
            assert!(matches!(parsed.command, Command::BalanceSheet));
        }
    }

    #[test]
    fn pair_accepts_missing_interactive_values_and_names_the_url_clearly() {
        let parsed = Args::try_parse_from(["bitgarth", "pair"]);
        assert!(parsed.is_ok());
        let Ok(parsed) = parsed else { return };
        assert!(matches!(
            parsed.command,
            Command::Pair {
                bitgarth_url: None,
                allow_insecure_http: false
            }
        ));

        let mut command = Args::command();
        let help = command
            .find_subcommand_mut("pair")
            .map(|pair| pair.render_help().to_string());
        assert!(
            help.as_deref().is_some_and(|help| {
                help.contains("[BITGARTH_URL]") && !help.contains("ORIGIN")
            })
        );
    }

    #[test]
    fn pair_defers_missing_http_consent_but_rejects_a_flag_for_https() {
        let http = Args::try_parse_from([
            "bitgarth",
            "--profile",
            "personal",
            "pair",
            "http://127.0.0.1:8080/path",
        ])
        .and_then(|args| {
            args.validate()?;
            Ok(args)
        });
        assert!(http.is_ok());

        let invalid_https = Args::try_parse_from([
            "bitgarth",
            "--profile",
            "personal",
            "pair",
            "https://example.com/path",
            "--allow-insecure-http",
        ])
        .and_then(|args| {
            args.validate()?;
            Ok(args)
        });
        assert_eq!(invalid_https.err().map(|error| error.exit_code()), Some(2));

        let duplicate = Args::try_parse_from([
            "bitgarth",
            "--profile",
            "one",
            "pair",
            "--profile",
            "two",
            "https://example.com",
        ])
        .and_then(|args| {
            args.validate()?;
            Ok(args)
        });
        assert_eq!(duplicate.err().map(|error| error.exit_code()), Some(2));
    }

    #[test]
    fn insecure_transport_consent_is_exact() {
        let cases = [
            (
                vec![
                    "bitgarth",
                    "--profile",
                    "p",
                    "pair",
                    "http://127.0.0.1:8080",
                ],
                true,
            ),
            (
                vec![
                    "bitgarth",
                    "--profile",
                    "p",
                    "pair",
                    "http://127.0.0.1:8080",
                    "--allow-insecure-http",
                ],
                true,
            ),
            (
                vec![
                    "bitgarth",
                    "--profile",
                    "p",
                    "pair",
                    "https://example.com",
                    "--allow-insecure-http",
                ],
                false,
            ),
        ];

        for (argv, expected) in cases {
            let result = Args::try_parse_from(argv).and_then(|args| {
                args.validate()?;
                Ok(args)
            });
            assert_eq!(result.is_ok(), expected);
        }
    }

    #[test]
    fn missing_profile_name_is_usage_error() {
        let result = Args::try_parse_from(["bitgarth", "profile", "remove"]);
        assert!(result.is_err());
        let Err(error) = result else {
            return;
        };

        assert_eq!(error.exit_code(), 2);
    }

    #[test]
    fn invalid_profile_name_is_usage_error() {
        let error = Args::try_parse_from(["bitgarth", "profile", "remove", " bad"]);
        assert_eq!(error.err().map(|error| error.exit_code()), Some(2));
    }

    #[test]
    fn profile_management_rejects_the_global_selector() {
        for argv in [
            vec!["bitgarth", "--profile", "selector", "profile", "list"],
            vec![
                "bitgarth",
                "--profile",
                "selector",
                "profile",
                "remove",
                "target",
            ],
            vec![
                "bitgarth",
                "--profile",
                "selector",
                "profile",
                "rename",
                "old",
                "new",
            ],
        ] {
            let parsed = Args::try_parse_from(argv);
            assert!(parsed.is_ok());
            let Ok(parsed) = parsed else { continue };
            assert_eq!(
                parsed.validate().err().map(|error| error.exit_code()),
                Some(2)
            );
        }
    }

    #[test]
    fn command_definition_is_valid() {
        Args::command().debug_assert();
    }
}
