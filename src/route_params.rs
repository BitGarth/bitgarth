use dioxus::prelude::dioxus_router::routable::{FromRouteSegments, ToRouteSegments};
use std::{fmt, str::FromStr};

// Dioxus 0.7 generates an error constructor for each path parameter. For
// String and Vec<String> its argument is Infallible, making the call
// unreachable on Rust 1.100. Keep accepting all decoded text while giving
// the generated parser an inhabited error type.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PairingLoginCode(pub(crate) String);

impl FromStr for PairingLoginCode {
    type Err = &'static str;

    fn from_str(code: &str) -> Result<Self, Self::Err> {
        Ok(Self(code.to_owned()))
    }
}

impl fmt::Display for PairingLoginCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UnmatchedSegments(pub(crate) Vec<String>);

impl FromRouteSegments for UnmatchedSegments {
    type Err = &'static str;

    fn from_route_segments(segments: &[&str]) -> Result<Self, Self::Err> {
        Ok(Self(segments.iter().map(|s| (*s).to_owned()).collect()))
    }
}

impl ToRouteSegments for UnmatchedSegments {
    fn display_route_segments(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display_route_segments(f)
    }
}
