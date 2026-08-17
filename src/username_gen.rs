use std::fmt;

// ============ Word Lists ============

const ADJECTIVES: &[&str] = &[
    "agile",
    "alpine",
    "amber",
    "ancient",
    "aqua",
    "arc",
    "arctic",
    "astral",
    "atomic",
    "aurora",
    "azure",
    "blazing",
    "bold",
    "boreal",
    "brave",
    "bright",
    "bronze",
    "carbon",
    "celestial",
    "chrome",
    "cipher",
    "civic",
    "cobalt",
    "coral",
    "cosmic",
    "crimson",
    "crystal",
    "cyber",
    "dapper",
    "daring",
    "dawn",
    "deep",
    "delta",
    "digital",
    "distant",
    "drift",
    "dual",
    "dusk",
    "dynamic",
    "echo",
    "elder",
    "electric",
    "emerald",
    "ember",
    "epic",
    "ethereal",
    "fable",
    "fern",
    "ferric",
    "fierce",
    "flash",
    "flint",
    "flora",
    "flying",
    "forge",
    "fossil",
    "frost",
    "galactic",
    "garnet",
    "gentle",
    "gilded",
    "glacier",
    "gleam",
    "glide",
    "golden",
    "granite",
    "graphite",
    "gravity",
    "grove",
    "halo",
    "harbor",
    "harmonic",
    "haven",
    "haze",
    "heroic",
    "hidden",
    "hollow",
    "horizon",
    "humble",
    "hushed",
    "hyper",
    "ice",
    "ignite",
    "indie",
    "indigo",
    "inner",
    "ionic",
    "iron",
    "ivory",
    "jade",
    "jasper",
    "keen",
    "kinetic",
    "lapis",
    "laser",
    "lattice",
    "lavish",
    "lemon",
    "light",
    "lime",
    "linear",
    "lofty",
    "lone",
    "lotus",
    "lucid",
    "lunar",
    "lush",
    "lyric",
    "mango",
    "maple",
    "marble",
    "marine",
    "marsh",
    "matte",
    "meadow",
    "mesa",
    "metal",
    "metro",
    "mighty",
    "mineral",
    "mint",
    "misty",
    "mocha",
    "modern",
    "molten",
    "mosaic",
    "mossy",
    "mystic",
    "native",
    "navy",
    "nebula",
    "neon",
    "nimble",
    "noble",
    "nomad",
    "nordic",
    "nova",
    "oak",
    "oaken",
    "obsidian",
    "ocean",
    "olive",
    "onyx",
    "opal",
    "orbit",
    "orchid",
    "outer",
    "oxide",
    "pacific",
    "pale",
    "palm",
    "pastel",
    "peak",
    "pearl",
    "phantom",
    "pine",
    "pixel",
    "plaid",
    "plasma",
    "polar",
    "polished",
    "prism",
    "pristine",
    "pulse",
    "pure",
    "quartz",
    "quiet",
    "radiant",
    "rapid",
    "raven",
    "ready",
    "rebel",
    "reef",
    "regal",
    "remote",
    "retro",
    "ridge",
    "ripple",
    "rising",
    "river",
    "robin",
    "robust",
    "rocky",
    "rooted",
    "royal",
    "ruby",
    "runic",
    "rustic",
    "rusty",
    "sage",
    "sahara",
    "sandstone",
    "satin",
    "scarlet",
    "scenic",
    "serene",
    "shadow",
    "sharp",
    "shelter",
    "sierra",
    "signal",
    "silent",
    "silk",
    "silver",
    "sleek",
    "slick",
    "solar",
    "solid",
    "sonic",
    "spark",
    "spectra",
    "spire",
    "spruce",
    "stark",
    "steady",
    "steel",
    "stellar",
    "stone",
    "storm",
    "strand",
    "stride",
    "subtle",
    "summit",
    "sunlit",
    "super",
    "surge",
    "swift",
    "synth",
    "tactic",
    "tango",
    "teal",
    "terra",
    "thrift",
    "thunder",
    "tidal",
    "timber",
    "titan",
    "topaz",
    "torch",
    "trace",
    "trail",
    "tranquil",
    "trek",
    "tropic",
    "true",
    "turbo",
    "twilight",
    "ultra",
    "umbra",
    "unified",
    "upper",
    "urban",
    "vale",
    "valid",
    "vapor",
    "velvet",
    "venture",
    "verdant",
    "vertex",
    "vibrant",
    "vivid",
    "void",
    "volt",
    "warp",
    "wave",
    "whisper",
    "wild",
    "willow",
    "winter",
    "wired",
    "wise",
    "wonder",
    "zenith",
    "zephyr",
    "zero",
    "zinc",
];

const NOUNS: &[&str] = &[
    "ace",
    "anchor",
    "anvil",
    "apex",
    "archer",
    "ark",
    "artisan",
    "atlas",
    "aurora",
    "badge",
    "badger",
    "baron",
    "bastion",
    "beacon",
    "bear",
    "bolt",
    "bridge",
    "captain",
    "cardinal",
    "cascade",
    "castle",
    "catalyst",
    "cedar",
    "centurion",
    "champion",
    "cipher",
    "citadel",
    "claw",
    "cliff",
    "coast",
    "comet",
    "compass",
    "condor",
    "coral",
    "coyote",
    "crane",
    "crest",
    "crow",
    "crown",
    "crusader",
    "crystal",
    "current",
    "dagger",
    "dancer",
    "dawn",
    "defender",
    "delta",
    "detective",
    "dingo",
    "dolphin",
    "dove",
    "dragon",
    "drake",
    "drift",
    "drum",
    "dusk",
    "eagle",
    "echo",
    "edge",
    "elder",
    "element",
    "elk",
    "ember",
    "engine",
    "envoy",
    "falcon",
    "fern",
    "ferret",
    "flame",
    "flare",
    "flash",
    "flint",
    "forge",
    "fort",
    "fossil",
    "fox",
    "frontier",
    "frost",
    "fury",
    "garden",
    "garnet",
    "gate",
    "gazer",
    "gecko",
    "ghost",
    "glacier",
    "glider",
    "globe",
    "golem",
    "granite",
    "griffin",
    "grove",
    "guard",
    "guide",
    "gull",
    "hare",
    "harbor",
    "hawk",
    "helix",
    "herald",
    "hermit",
    "heron",
    "horizon",
    "hornet",
    "hound",
    "hunter",
    "ibex",
    "jaguar",
    "javelin",
    "jewel",
    "kestrel",
    "knight",
    "lance",
    "lancer",
    "lantern",
    "lark",
    "leopard",
    "light",
    "lion",
    "lodge",
    "lotus",
    "lynx",
    "mage",
    "magnet",
    "mantis",
    "maple",
    "marble",
    "marshal",
    "marten",
    "mason",
    "maverick",
    "mesa",
    "meteor",
    "mint",
    "mirror",
    "monk",
    "moon",
    "moose",
    "moth",
    "muse",
    "mystic",
    "nebula",
    "nettle",
    "nomad",
    "oasis",
    "oracle",
    "orchid",
    "osprey",
    "otter",
    "outpost",
    "owl",
    "paladin",
    "panther",
    "parrot",
    "path",
    "patrol",
    "pebble",
    "pelican",
    "phoenix",
    "pike",
    "pilot",
    "pine",
    "pioneer",
    "pixel",
    "plover",
    "point",
    "portal",
    "prism",
    "prowler",
    "pulse",
    "quail",
    "quest",
    "rain",
    "ranger",
    "raptor",
    "raven",
    "ray",
    "reef",
    "rider",
    "ridge",
    "river",
    "robin",
    "rocket",
    "rover",
    "ruby",
    "sage",
    "scout",
    "seal",
    "seed",
    "sentinel",
    "shade",
    "shell",
    "shield",
    "shore",
    "signal",
    "skipper",
    "sky",
    "slate",
    "sloth",
    "smith",
    "snipe",
    "solar",
    "soul",
    "spark",
    "sparrow",
    "spear",
    "sphinx",
    "spider",
    "spirit",
    "sprout",
    "spruce",
    "squall",
    "squire",
    "stag",
    "star",
    "steel",
    "stone",
    "storm",
    "strand",
    "stride",
    "summit",
    "sun",
    "surf",
    "swan",
    "swift",
    "sword",
    "talon",
    "tempest",
    "terra",
    "thistle",
    "thorn",
    "thrush",
    "thunder",
    "tide",
    "tiger",
    "timber",
    "titan",
    "torch",
    "totem",
    "tower",
    "tracer",
    "trail",
    "trident",
    "turtle",
    "tusk",
    "vale",
    "valley",
    "vanguard",
    "vault",
    "vessel",
    "vine",
    "viper",
    "vision",
    "vortex",
    "voyager",
    "walker",
    "wanderer",
    "warden",
    "watcher",
    "wave",
    "weaver",
    "whale",
    "whisper",
    "willow",
    "wind",
    "wing",
    "wolf",
    "wren",
    "zenith",
];

// ============ Error Type ============

#[derive(Debug)]
pub(crate) struct UsernameGenError(getrandom::Error);

impl fmt::Display for UsernameGenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Failed to generate random bytes for username: {}",
            self.0
        )
    }
}

// ============ Random Helpers ============

/// Fill a buffer with random bytes using getrandom.
fn fill_random(buf: &mut [u8]) -> Result<(), UsernameGenError> {
    getrandom::fill(buf).map_err(UsernameGenError)
}

/// Generate a random index in [0, len) using rejection sampling to avoid modulo bias.
fn random_index(len: usize) -> Result<usize, UsernameGenError> {
    debug_assert!(len > 0, "random_index called with len == 0");
    debug_assert!(len <= 65536, "random_index only supports len <= 65536");

    // Use 2 bytes (u16) for the random value, supporting lists up to 65536 entries.
    let range = 65536_usize;
    let limit = range - (range % len);
    let mut buf = [0u8; 2];

    loop {
        fill_random(&mut buf)?;
        let val = u16::from_le_bytes(buf) as usize;
        if val < limit {
            return Ok(val % len);
        }
    }
}

// ============ Pure Composition ============

/// Compose a username from the given adjective and noun indices.
/// Pure function: deterministic for given indices.
fn compose_username(adj_index: usize, noun_index: usize) -> String {
    let adj = ADJECTIVES[adj_index % ADJECTIVES.len()];
    let noun = NOUNS[noun_index % NOUNS.len()];
    format!("{adj}-{noun}")
}

// ============ Public Generation ============

/// Generate a random username in the form "adjective-noun".
pub(crate) fn generate_username() -> Result<String, UsernameGenError> {
    let adj_idx = random_index(ADJECTIVES.len())?;
    let noun_idx = random_index(NOUNS.len())?;
    Ok(compose_username(adj_idx, noun_idx))
}

#[cfg(all(test, not(bitgarth_db_unit_only)))]
mod tests {
    use super::*;

    // ---- Word list validation ----

    fn is_valid_word(word: &str) -> bool {
        word.len() >= 3 && word.len() <= 15 && word.chars().all(|c| c.is_ascii_lowercase())
    }

    #[test]
    fn test_all_adjectives_valid() {
        for (i, adj) in ADJECTIVES.iter().enumerate() {
            assert!(
                is_valid_word(adj),
                "ADJECTIVES[{i}] = \"{adj}\" is invalid (must be 3-15 lowercase ASCII chars)"
            );
        }
    }

    #[test]
    fn test_all_nouns_valid() {
        for (i, noun) in NOUNS.iter().enumerate() {
            assert!(
                is_valid_word(noun),
                "NOUNS[{i}] = \"{noun}\" is invalid (must be 3-15 lowercase ASCII chars)"
            );
        }
    }

    #[test]
    fn test_word_list_sizes() {
        assert!(
            ADJECTIVES.len() >= 200,
            "Expected at least 200 adjectives, got {}",
            ADJECTIVES.len()
        );
        assert!(
            NOUNS.len() >= 200,
            "Expected at least 200 nouns, got {}",
            NOUNS.len()
        );
    }

    #[test]
    fn test_all_combinations_under_max_length() {
        let max_adj = ADJECTIVES.iter().map(|w| w.len()).max().unwrap_or(0);
        let max_noun = NOUNS.iter().map(|w| w.len()).max().unwrap_or(0);
        let max_total = max_adj + 1 + max_noun; // +1 for hyphen
        assert!(
            max_total <= 64,
            "Longest possible username is {max_total} chars (adj={max_adj} + 1 + noun={max_noun}), exceeds 64"
        );
    }

    // ---- Pure composition ----

    #[test]
    fn test_compose_username_basic() {
        let result = compose_username(0, 0);
        assert_eq!(result, format!("{}-{}", ADJECTIVES[0], NOUNS[0]));
    }

    #[test]
    fn test_compose_username_wraps() {
        let adj_count = ADJECTIVES.len();
        let noun_count = NOUNS.len();
        let result = compose_username(adj_count + 3, noun_count + 5);
        assert_eq!(result, format!("{}-{}", ADJECTIVES[3], NOUNS[5]));
    }

    // ---- Random index ----

    #[test]
    fn test_random_index_in_bounds() {
        for len in [1, 2, 10, 100, 250, 256, 300, 500] {
            for _ in 0..50 {
                let idx = random_index(len).expect("random_index should succeed");
                assert!(idx < len, "random_index({len}) returned {idx}");
            }
        }
    }

    // ---- generate_username ----

    #[test]
    fn test_generate_username_format() {
        let name = generate_username().expect("should succeed");
        assert!(name.contains('-'), "Expected hyphen in \"{name}\"");
        let parts: Vec<&str> = name.splitn(2, '-').collect();
        assert_eq!(parts.len(), 2);
        assert!(
            parts[0].chars().all(|c| c.is_ascii_lowercase()),
            "Adjective part has invalid chars: \"{}\"",
            parts[0]
        );
        assert!(
            parts[1].chars().all(|c| c.is_ascii_lowercase()),
            "Noun part has invalid chars: \"{}\"",
            parts[1]
        );
    }

    #[test]
    fn test_generate_username_within_validation_bounds() {
        for _ in 0..100 {
            let name = generate_username().expect("should succeed");
            assert!(
                name.len() <= 64,
                "Generated username \"{name}\" exceeds 64 chars"
            );
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "Generated username \"{name}\" has invalid chars"
            );
        }
    }
}
