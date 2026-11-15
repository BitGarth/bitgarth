use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default())
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Yellow.on_default())
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD));

const ROOT_AFTER_HELP: &str = "\
\x1b[1;32mGetting started:\x1b[0m
  bitgarth auth login https://my.bitgarth.app      Pair this computer with your account
  bitgarth balance                                 What you own, per wallet
  bitgarth account add bitcoin bc1q…               Track a Bitcoin address or xpub
  bitgarth settings set price-fetching on          Show values using CoinGecko prices

\x1b[1;32mFor scripts and AI agents:\x1b[0m
  • Add --json to any command for complete, machine-readable output on stdout.
    Progress, prompts and warnings go to stderr.
  • Commands never prompt when stdin is not a terminal; pass --yes to confirm
    deletions and pipe secrets on stdin (see `bitgarth secret set --help`).
  • Wallets and accounts can be named by ID, unique ID prefix, or exact name.
  • `bitgarth api` sends raw requests to the /api/v1 endpoints.

\x1b[1;32mExit codes:\x1b[0m
  0 success   1 failure   2 invalid usage   4 not logged in or access denied

\x1b[1;32mEnvironment:\x1b[0m
  BITGARTH_PROFILE   Profile to use, like --profile
  NO_COLOR           Disable colored output";

/// Track your crypto portfolio on BitGarth from the command line.
///
/// Works with the hosted service at https://my.bitgarth.app and with
/// self-hosted BitGarth servers.
#[derive(Parser)]
#[command(
    name = "bitgarth",
    version,
    styles = STYLES,
    after_help = ROOT_AFTER_HELP,
    disable_help_subcommand = true
)]
pub(crate) struct Args {
    #[command(flatten)]
    pub(crate) global: GlobalArgs,

    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(ClapArgs, Clone)]
pub(crate) struct GlobalArgs {
    /// Saved login to use (see `bitgarth auth status`)
    #[arg(
        long,
        global = true,
        env = "BITGARTH_PROFILE",
        value_parser = parse_profile_name
    )]
    pub(crate) profile: Option<String>,

    /// Print complete machine-readable JSON instead of formatted output
    #[arg(long, global = true)]
    pub(crate) json: bool,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Log in to a BitGarth server, switch accounts, or log out
    #[command(subcommand, display_order = 1)]
    Auth(AuthCommand),

    /// Show what you own: balances and values for every wallet
    #[command(visible_aliases = ["bs", "balancesheet"], display_order = 2, after_help = "\
Examples:
  bitgarth balance
  bitgarth balance --wallet \"Cold storage\"
  bitgarth balance --json | jq '.value_summary'")]
    Balance {
        /// Only show this wallet
        #[arg(long, value_name = "WALLET")]
        wallet: Option<String>,
    },

    /// List, inspect, rename and delete wallets
    #[command(subcommand, visible_alias = "wallets", display_order = 3)]
    Wallet(WalletCommand),

    /// Add and manage accounts: addresses, xpubs and manual assets
    #[command(subcommand, visible_alias = "accounts", display_order = 4)]
    Account(AccountCommand),

    /// Holdings reports for a date range
    #[command(subcommand, display_order = 5)]
    Report(ReportCommand),

    /// Fetch the latest balances and transactions from the blockchain
    #[command(subcommand, display_order = 6)]
    Sync(SyncCommand),

    /// Price history and your own price entries
    #[command(subcommand, visible_alias = "prices", display_order = 7)]
    Price(PriceCommand),

    /// Search the asset catalog for manual accounts
    #[command(subcommand, visible_alias = "assets", display_order = 8)]
    Asset(AssetCommand),

    /// View and change settings such as currency and CoinGecko prices
    #[command(subcommand, visible_alias = "config", display_order = 9)]
    Settings(SettingsCommand),

    /// Store or remove API keys such as your Etherscan key
    #[command(subcommand, visible_alias = "secrets", display_order = 10)]
    Secret(SecretCommand),

    /// Export wallet records to a new password-encrypted ZIP
    #[command(subcommand)]
    Export(WalletDataCommand),

    /// Import wallet records from a password-encrypted ZIP
    #[command(subcommand)]
    Import(WalletDataCommand),

    /// Make an authenticated request to the BitGarth API
    #[command(
        display_order = 11,
        after_help = "\
The path is relative to /api/v1/. The response is printed as JSON.

Endpoints:
  GET    me, wallets, wallets/{id}/report, reports/holdings, sync, settings,
         accounts/{id}/addresses?scheme=…, accounts/{id}/transactions,
         assets/search?query=…, prices/history, prices/overrides
  POST   accounts/bitcoin-address, accounts/ethereum-address, accounts/xpub,
         accounts/manual, accounts/{id}/move, accounts/{id}/balance-assertions,
         xpubs/inspect, sync, exports/wallet-data, imports/wallet-data
  PATCH  wallets/{id}, accounts/{id}
  PUT    settings/{key}, secrets/{name}, prices/overrides,
         accounts/{id}/balance-assertions/{assertion-id}
  DELETE wallets/{id}, accounts/{id}, secrets/{name}, prices/overrides,
         accounts/{id}/balance-assertions/{assertion-id}, clients/self

Examples:
  bitgarth api me
  bitgarth api wallets -q '.wallets[].label'
  bitgarth api -X PATCH wallets/01J… -f label='Cold storage'
  echo '{\"value\": \"EUR\"}' | bitgarth api -X PUT settings/currency --input -"
    )]
    Api(ApiArgs),
}

#[derive(Subcommand)]
pub(crate) enum WalletDataCommand {
    /// Transfer wallets, accounts, addresses, and manual balance entries
    #[command(
        after_help = "Use a full-access profile. Enter the archive password at the prompt, or pipe one line on stdin.
Export creates a new file. Import merges wallet records and skips settings, API keys, and subscription transfers.

Examples:
  bitgarth export wallet-data portfolio.zip
  bitgarth import wallet-data portfolio.zip"
    )]
    WalletData {
        /// Path to the encrypted wallet-data ZIP
        file: std::path::PathBuf,
    },
}

// ============ auth ============

#[derive(Subcommand)]
pub(crate) enum AuthCommand {
    /// Pair this computer with your BitGarth account
    #[command(
        visible_alias = "pair",
        after_help = "\
How it works:
  1. This command prints a link and a code.
  2. Open the link in a browser where you are signed in to BitGarth, check
     that the code matches, and approve.
  3. The command finishes on its own (it waits up to 10 minutes).
The login is saved as a profile named after the server (change it with
--profile) and becomes the default. The approval stores a Client Key for
this computer in your user config directory. Revoke it any time with `bitgarth auth logout` or in the web app
under Settings > Paired Clients.

AI agents: run this in the background (it blocks until approved), show the
user the link and code from stderr, and wait for the command to exit 0.

Examples:
  bitgarth auth login https://my.bitgarth.app
  bitgarth auth login https://bitgarth.example.com --profile home --read-only
  bitgarth auth login http://127.0.0.1:8080 --allow-insecure-http"
    )]
    Login {
        /// Address of your BitGarth server, as shown in your browser
        #[arg(value_name = "URL")]
        url: Option<String>,
        /// Only allow reading; changes such as adding accounts will be refused
        #[arg(long)]
        read_only: bool,
        /// Allow a plain-HTTP address, such as a server on this computer
        #[arg(long)]
        allow_insecure_http: bool,
    },

    /// Revoke this computer's access and forget the login
    #[command(after_help = "\
Examples:
  bitgarth auth logout
  bitgarth auth logout --profile work")]
    Logout {
        /// Only forget the login here; leave its access active on the server
        #[arg(long)]
        local_only: bool,
    },

    /// Show saved logins and check that the active one still works
    Status,

    /// Make a saved login the default
    Switch {
        /// Profile to use by default
        #[arg(value_parser = parse_profile_name)]
        name: String,
    },

    /// Rename a saved login
    Rename {
        /// Current profile name
        #[arg(value_parser = parse_profile_name)]
        old: String,
        /// New profile name
        #[arg(value_parser = parse_profile_name)]
        new: String,
    },
}

// ============ wallet ============

#[derive(Subcommand)]
pub(crate) enum WalletCommand {
    /// List wallets with their account count and value
    #[command(visible_alias = "ls")]
    List,

    /// Show a wallet's accounts and balances
    #[command(after_help = "\
Examples:
  bitgarth wallet view \"Cold storage\"
  bitgarth wallet view 01JC2X")]
    View {
        /// Wallet name, ID or unique ID prefix
        wallet: String,
    },

    /// Rename a wallet
    Rename {
        /// Wallet name, ID or unique ID prefix
        wallet: String,
        /// New name
        name: String,
    },

    /// Delete a wallet and all of its accounts
    #[command(visible_alias = "rm")]
    Delete {
        /// Wallet name, ID or unique ID prefix
        wallet: String,
        /// Skip the confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

// ============ account ============

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// List accounts across wallets
    #[command(visible_alias = "ls")]
    List {
        /// Only list accounts in this wallet
        #[arg(long, value_name = "WALLET")]
        wallet: Option<String>,
    },

    /// Show an account's balance, sync state and recent activity
    View {
        /// Account name, ID or unique ID prefix
        account: String,
    },

    /// Track a new address, xpub or manual asset
    #[command(subcommand)]
    Add(AddAccountCommand),

    /// Rename an account
    Rename {
        /// Account name, ID or unique ID prefix
        account: String,
        /// New name
        name: String,
    },

    /// Move an account to another wallet or a new one
    #[command(after_help = "\
Examples:
  bitgarth account move \"Savings\" --to \"Cold storage\"
  bitgarth account move 01JC2X --new-wallet \"Hardware wallet\"")]
    Move {
        /// Account name, ID or unique ID prefix
        account: String,
        #[command(flatten)]
        destination: Destination,
    },

    /// Stop tracking an account and delete its history
    #[command(visible_alias = "rm")]
    Delete {
        /// Account name, ID or unique ID prefix
        account: String,
        /// Skip the confirmation prompt
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// List the addresses an xpub account has derived
    Addresses {
        /// Account name, ID or unique ID prefix
        account: String,
        /// Page number
        #[arg(long, default_value_t = 1)]
        page: u32,
    },

    /// List an account's transactions, or a manual account's balance entries
    #[command(visible_aliases = ["tx", "txs"], after_help = "\
Examples:
  bitgarth account transactions Savings
  bitgarth account tx Savings --page 2 --oldest-first
  bitgarth account tx Savings --from 2026-01-01 --to 2026-03-31")]
    Transactions {
        /// Account name, ID or unique ID prefix
        account: String,
        /// Page number
        #[arg(long, default_value_t = 1)]
        page: u32,
        /// Show the oldest transactions first
        #[arg(long)]
        oldest_first: bool,
        /// Only include transactions on or after this date (YYYY-MM-DD)
        #[arg(long, value_name = "DATE")]
        from: Option<String>,
        /// Only include transactions on or before this date (YYYY-MM-DD)
        #[arg(long, value_name = "DATE")]
        to: Option<String>,
    },

    /// Record a manual account's balance on a date
    #[command(after_help = "\
Manual accounts have no blockchain to read from, so you record what you hold.
Setting a balance on a date that already has one replaces it.

Examples:
  bitgarth account set-balance \"Gold\" 12.5
  bitgarth account set-balance \"Gold\" 12.5 --date 2026-01-31 --note \"Vault audit\"")]
    SetBalance {
        /// Manual account name, ID or unique ID prefix
        account: String,
        /// Balance in the asset's unit, such as 12.5
        amount: String,
        /// Date the balance applies to (YYYY-MM-DD); defaults to today
        #[arg(long)]
        date: Option<String>,
        /// Optional note
        #[arg(long)]
        note: Option<String>,
    },

    /// Remove a manual account's recorded balance for a date
    UnsetBalance {
        /// Manual account name, ID or unique ID prefix
        account: String,
        /// Date of the balance entry to remove (YYYY-MM-DD)
        #[arg(long)]
        date: String,
    },
}

#[derive(ClapArgs, Clone)]
#[group(required = true, multiple = false)]
pub(crate) struct Destination {
    /// Existing wallet to move the account into
    #[arg(long, value_name = "WALLET")]
    pub(crate) to: Option<String>,
    /// Create a wallet with this name and move the account into it
    #[arg(long, value_name = "NAME")]
    pub(crate) new_wallet: Option<String>,
}

#[derive(ClapArgs, Clone)]
#[group(multiple = false)]
pub(crate) struct WalletChoice {
    /// Add to this existing wallet
    #[arg(long, value_name = "WALLET")]
    pub(crate) wallet: Option<String>,
    /// Create a wallet with this name (the default, named after the account)
    #[arg(long, value_name = "NAME")]
    pub(crate) new_wallet: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum AddAccountCommand {
    /// Track a Bitcoin address or extended public key (xpub/ypub/zpub)
    #[command(
        visible_alias = "btc",
        after_help = "\
Without --wallet or --new-wallet, a new wallet is created.

For an extended public key, BitGarth checks which address type has activity
and picks it; use --scheme to choose yourself.

Examples:
  bitgarth account add bitcoin bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq
  bitgarth account add bitcoin zpub6r… --wallet \"Cold storage\" --name Savings
  bitgarth account add btc xpub6C… --scheme native-segwit --new-wallet Trezor"
    )]
    Bitcoin {
        /// Bitcoin address, or an xpub/ypub/zpub extended public key
        address_or_xpub: String,
        #[command(flatten)]
        wallet: WalletChoice,
        /// Account name (default: "Bitcoin Account N")
        #[arg(long)]
        name: Option<String>,
        /// Address type for an extended public key
        #[arg(long, value_enum)]
        scheme: Option<BitcoinScheme>,
        /// Bitcoin network of a single address
        #[arg(long, value_enum, default_value_t = BitcoinNetwork::Mainnet)]
        network: BitcoinNetwork,
    },

    /// Track an Ethereum address
    #[command(
        visible_alias = "eth",
        after_help = "\
Ethereum sync uses Etherscan; set your key with `bitgarth secret set etherscan-api-key`.

Examples:
  bitgarth account add ethereum 0x742d35Cc6634C0532925a3b844Bc454e4438f44e
  bitgarth account add eth 0x742d… --wallet \"Hot wallet\" --name Trading"
    )]
    Ethereum {
        /// Ethereum address (0x…)
        address: String,
        #[command(flatten)]
        wallet: WalletChoice,
        /// Account name (default: "Ethereum Account N")
        #[arg(long)]
        name: Option<String>,
    },

    /// Track an asset by hand, such as a token on an exchange or gold
    #[command(after_help = "\
ASSET is an exact asset from `bitgarth asset search`: its ID (asset@network)
or a search that matches exactly one asset. Record holdings afterwards with
`bitgarth account set-balance`.

Examples:
  bitgarth asset search usdc
  bitgarth account add manual usd-coin@ethereum-mainnet --new-wallet Exchange
  bitgarth account add manual \"Tether\" --wallet Exchange --name \"USDT on Kraken\"")]
    Manual {
        /// Asset ID (asset@network) or a search matching exactly one asset
        asset: String,
        #[command(flatten)]
        wallet: WalletChoice,
        /// Account name (default: "UNIT Account N")
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum BitcoinScheme {
    /// P2PKH, addresses starting with 1
    Legacy,
    /// P2SH-P2WPKH, addresses starting with 3
    NestedSegwit,
    /// P2WPKH, addresses starting with bc1q
    NativeSegwit,
}

impl BitcoinScheme {
    pub(crate) const fn api_name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::NestedSegwit => "nested_segwit",
            Self::NativeSegwit => "native_segwit",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum BitcoinNetwork {
    Mainnet,
    Testnet,
    Signet,
    Regtest,
}

impl BitcoinNetwork {
    pub(crate) const fn api_name(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Signet => "signet",
            Self::Regtest => "regtest",
        }
    }
}

// ============ report ============

#[derive(Subcommand)]
pub(crate) enum ReportCommand {
    /// Opening and closing value of every wallet over a period
    #[command(after_help = "\
Defaults to this year so far. Dates are YYYY-MM-DD in your BitGarth timezone.

Examples:
  bitgarth report holdings
  bitgarth report holdings --from 2025-01-01 --to 2025-12-31")]
    Holdings {
        #[command(flatten)]
        range: DateRange,
    },

    /// Opening and closing balance of each account in a wallet
    Wallet {
        /// Wallet name, ID or unique ID prefix
        wallet: String,
        #[command(flatten)]
        range: DateRange,
    },
}

#[derive(ClapArgs, Clone)]
pub(crate) struct DateRange {
    /// First day of the period (YYYY-MM-DD)
    #[arg(long, value_name = "DATE")]
    pub(crate) from: Option<String>,
    /// Last day of the period (YYYY-MM-DD)
    #[arg(long, value_name = "DATE")]
    pub(crate) to: Option<String>,
}

// ============ sync ============

#[derive(Subcommand)]
pub(crate) enum SyncCommand {
    /// Show when each account last synced and whether it worked
    Status,

    /// Start a sync now
    #[command(after_help = "\
Examples:
  bitgarth sync run
  bitgarth sync run Savings --wait")]
    Run {
        /// Only sync this account (name, ID or unique ID prefix)
        account: Option<String>,
        /// Wait until the sync finishes and show the result
        #[arg(long)]
        wait: bool,
    },
}

// ============ price ============

#[derive(Subcommand)]
pub(crate) enum PriceCommand {
    /// Daily prices for an asset you hold
    #[command(after_help = "\
Examples:
  bitgarth price history bitcoin
  bitgarth price history ethereum --from 2026-01-01 --source yours")]
    History {
        /// Asset ID such as bitcoin; defaults to your first held asset
        asset: Option<String>,
        #[command(flatten)]
        range: DateRange,
        /// Page number
        #[arg(long, default_value_t = 1)]
        page: u32,
        /// Show the oldest prices first
        #[arg(long)]
        oldest_first: bool,
        /// Which prices to show
        #[arg(long, value_enum, default_value_t = PriceSource::All)]
        source: PriceSource,
    },

    /// Record your own price for an asset at a moment in time
    #[command(after_help = "\
Your prices take precedence over CoinGecko in reports. Times are in your
BitGarth timezone unless an RFC 3339 timestamp includes an offset.

Examples:
  bitgarth price set bitcoin 61250.00 --at 2026-01-01
  bitgarth price set algorand 0.12 --at \"2026-03-31 17:00\" --currency USD --note \"Closing price\"")]
    Set {
        /// Asset ID such as bitcoin
        asset: String,
        /// Price of one unit
        price: String,
        /// Date, local date and time, or RFC 3339 timestamp from price history
        #[arg(long, value_name = "WHEN")]
        at: String,
        /// Quote currency; defaults to your BitGarth currency
        #[arg(long)]
        currency: Option<String>,
        /// Where the price came from
        #[arg(long)]
        note: Option<String>,
    },

    /// Remove one of your own prices
    #[command(visible_alias = "rm")]
    Delete {
        /// Asset ID such as bitcoin
        asset: String,
        /// Date, local date and time, or RFC 3339 timestamp from price history
        #[arg(long, value_name = "WHEN")]
        at: String,
        /// Quote currency; defaults to your BitGarth currency
        #[arg(long)]
        currency: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum PriceSource {
    /// Every source
    All,
    /// Only prices you entered
    Yours,
    /// Only CoinGecko prices
    Coingecko,
}

impl PriceSource {
    pub(crate) const fn api_name(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Yours => "YourPrices",
            Self::Coingecko => "CoinGecko",
        }
    }
}

// ============ asset ============

#[derive(Subcommand)]
pub(crate) enum AssetCommand {
    /// Find assets by name, ticker or network
    #[command(after_help = "\
Examples:
  bitgarth asset search usdc
  bitgarth asset search \"wrapped bitcoin\"")]
    Search {
        /// Words to search for
        #[arg(required = true, num_args = 1..)]
        query: Vec<String>,
    },
}

// ============ settings ============

#[derive(Subcommand)]
pub(crate) enum SettingsCommand {
    /// Show every setting
    #[command(visible_alias = "ls")]
    List,

    /// Print one setting's value
    Get {
        /// Setting name
        #[arg(value_enum)]
        key: SettingKey,
    },

    /// Change a setting
    #[command(after_help = "\
Settings:
  currency        Reporting currency, an ISO 4217 code such as EUR
  timezone        IANA timezone such as Europe/Amsterdam; used for reports
  number-format   dot_comma (1,234.56), comma_dot (1.234,56), comma_space (1 234,56)
  date-format     ymd_24, dmy_24 or mdy_12
  price-fetching  on/off: fetch market prices from CoinGecko
  mempool-url     Bitcoin data source, e.g. your own mempool instance
  etherscan-url   Etherscan API path (choose the provider origin in the web app)
  hledger-prefix  hledger account prefix such as assets:crypto

Examples:
  bitgarth settings set currency EUR
  bitgarth settings set price-fetching on
  bitgarth settings set mempool-url https://mempool.example.com")]
    Set {
        /// Setting name
        #[arg(value_enum)]
        key: SettingKey,
        /// New value
        value: String,
    },

    /// Restore a setting to its default
    #[command(after_help = "\
Only mempool-url, etherscan-url and hledger-prefix can be unset.

Example:
  bitgarth settings unset mempool-url")]
    Unset {
        /// Setting name
        #[arg(value_enum)]
        key: SettingKey,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SettingKey {
    Currency,
    Timezone,
    NumberFormat,
    DateFormat,
    PriceFetching,
    MempoolUrl,
    EtherscanUrl,
    HledgerPrefix,
}

impl SettingKey {
    pub(crate) const fn api_name(self) -> &'static str {
        match self {
            Self::Currency => "currency",
            Self::Timezone => "timezone",
            Self::NumberFormat => "number-format",
            Self::DateFormat => "date-format",
            Self::PriceFetching => "price-fetching",
            Self::MempoolUrl => "mempool-url",
            Self::EtherscanUrl => "etherscan-url",
            Self::HledgerPrefix => "hledger-prefix",
        }
    }
}

// ============ secret ============

#[derive(Subcommand)]
pub(crate) enum SecretCommand {
    /// Show which secrets are stored (never their values)
    #[command(visible_alias = "ls")]
    List,

    /// Store a secret
    #[command(after_help = "\
On a terminal you are prompted without echo. Otherwise the value is read
from stdin, so it never appears in your shell history or process list.

Secrets:
  etherscan-api-key   Needed to sync Ethereum accounts; checked with Etherscan when saved
  coingecko-api-key   Optional CoinGecko Pro key for higher price limits

Examples:
  bitgarth secret set etherscan-api-key
  op read op://vault/etherscan/key | bitgarth secret set etherscan-api-key")]
    Set {
        /// Which secret to store
        #[arg(value_enum)]
        name: SecretName,
    },

    /// Remove a stored secret
    #[command(visible_alias = "rm")]
    Delete {
        /// Which secret to remove
        #[arg(value_enum)]
        name: SecretName,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SecretName {
    EtherscanApiKey,
    CoingeckoApiKey,
}

impl SecretName {
    pub(crate) const fn api_name(self) -> &'static str {
        match self {
            Self::EtherscanApiKey => "etherscan-api-key",
            Self::CoingeckoApiKey => "coingecko-api-key",
        }
    }
}

// ============ api ============

#[derive(ClapArgs)]
pub(crate) struct ApiArgs {
    /// Endpoint path relative to /api/v1/, optionally with ?query
    pub(crate) path: String,
    /// HTTP method
    #[arg(short = 'X', long, default_value = "GET", value_parser = ["GET", "POST", "PUT", "PATCH", "DELETE"], ignore_case = true)]
    pub(crate) method: String,
    /// Add a string field to the JSON body (key=value)
    #[arg(short = 'f', long = "field", value_name = "KEY=VALUE")]
    pub(crate) fields: Vec<String>,
    /// Add a typed field: true, false, null, numbers and JSON parse as such
    #[arg(short = 'F', long = "raw-field", value_name = "KEY=VALUE")]
    pub(crate) raw_fields: Vec<String>,
    /// Read the JSON body from a file, or - for stdin
    #[arg(long, value_name = "FILE", conflicts_with_all = ["fields", "raw_fields"])]
    pub(crate) input: Option<String>,
    /// Print only the value at this path, such as .wallets[0].label
    #[arg(short = 'q', long = "query", value_name = "PATH")]
    pub(crate) query: Option<String>,
}

fn parse_profile_name(value: &str) -> Result<String, String> {
    crate::profiles::validate_profile_name(value)
        .map(|()| value.to_owned())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{AccountCommand, AddAccountCommand, Args, AuthCommand, Command};

    fn parse(argv: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(argv)
    }

    #[test]
    fn command_definition_is_valid() {
        Args::command().debug_assert();
    }

    #[test]
    fn every_command_and_argument_explains_itself() {
        fn undocumented(command: &clap::Command, path: &str, missing: &mut Vec<String>) {
            if command.get_about().is_none() {
                missing.push(path.to_owned());
            }
            for argument in command.get_arguments() {
                if argument.get_help().is_none() && argument.get_id() != "help" {
                    missing.push(format!("{path} <{}>", argument.get_id()));
                }
            }
            for subcommand in command.get_subcommands() {
                let path = format!("{path} {}", subcommand.get_name());
                undocumented(subcommand, &path, missing);
            }
        }
        let mut missing = Vec::new();
        undocumented(&Args::command(), "bitgarth", &mut missing);
        assert_eq!(missing, Vec::<String>::new());
    }

    #[test]
    fn login_has_pair_alias_and_global_flags_anywhere() {
        for argv in [
            vec![
                "bitgarth",
                "auth",
                "login",
                "https://example.com",
                "--read-only",
            ],
            vec![
                "bitgarth",
                "--json",
                "auth",
                "pair",
                "https://example.com",
                "--read-only",
            ],
        ] {
            let parsed = parse(&argv);
            assert!(matches!(
                parsed.map(|args| args.command),
                Ok(Command::Auth(AuthCommand::Login { read_only: true, url: Some(url), .. }))
                    if url == "https://example.com"
            ));
        }
        let parsed = parse(&["bitgarth", "wallet", "list", "--profile", "home", "--json"]);
        assert!(
            parsed.is_ok_and(
                |args| args.global.json && args.global.profile.as_deref() == Some("home")
            )
        );
    }

    #[test]
    fn balancesheet_aliases_reach_balance() {
        for alias in ["balance", "bs", "balancesheet"] {
            assert!(matches!(
                parse(&["bitgarth", alias]).map(|args| args.command),
                Ok(Command::Balance { wallet: None })
            ));
        }
    }

    #[test]
    fn add_account_wallet_choice_is_exclusive() {
        let both = parse(&[
            "bitgarth",
            "account",
            "add",
            "btc",
            "bc1q",
            "--wallet",
            "a",
            "--new-wallet",
            "b",
        ]);
        assert_eq!(both.err().map(|error| error.exit_code()), Some(2));
        let one = parse(&[
            "bitgarth",
            "account",
            "add",
            "eth",
            "0xabc",
            "--new-wallet",
            "Hot",
        ]);
        assert!(matches!(
            one.map(|args| args.command),
            Ok(Command::Account(AccountCommand::Add(AddAccountCommand::Ethereum { wallet, .. })))
                if wallet.new_wallet.as_deref() == Some("Hot")
        ));
    }

    #[test]
    fn move_requires_exactly_one_destination() {
        assert!(parse(&["bitgarth", "account", "move", "a"]).is_err());
        assert!(
            parse(&[
                "bitgarth",
                "account",
                "move",
                "a",
                "--to",
                "b",
                "--new-wallet",
                "c"
            ])
            .is_err()
        );
        assert!(parse(&["bitgarth", "account", "move", "a", "--to", "b"]).is_ok());
    }

    #[test]
    fn invalid_profile_name_is_usage_error() {
        let error = parse(&["bitgarth", "--profile", " bad", "auth", "status"]);
        assert_eq!(error.err().map(|error| error.exit_code()), Some(2));
    }
}
