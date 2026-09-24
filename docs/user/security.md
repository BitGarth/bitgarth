# Security & Privacy

BitGarth is built for people who do not want their financial life pooled into someone else's database.

Your wallet data, transaction history, labels, settings, and saved API keys live in your encrypted user database. Transaction exports and backups are encrypted by default, although you can export them unencrypted when you need to.

BitGarth does not need your name, email address, seed phrase, private keys, or exchange login passwords to use the app. It only needs public wallet information, such as Bitcoin addresses, xpubs, and Ethereum addresses. That is enough to sync transaction history without ever being able to move funds.

## What BitGarth Protects Against

BitGarth reduces the risk of exposing your portfolio to hosted crypto trackers, tax SaaS platforms, cloud aggregators, and data brokers.

Because private financial data stays in your app instance, there is no central BitGarth account database containing everyone's wallet balances, transaction histories, labels, or identity details. There is no marketing profile to build, and no email list required to use the app.

The app is designed around a simple rule: if BitGarth does not need a piece of information, it should not collect it.

## What BitGarth Does Not Need For Wallet Sync

BitGarth does not need:

- Seed phrases
- Private keys
- Signing keys
- Exchange login passwords
- Custodial withdrawal access
- Your legal name or address for wallet sync
- Your email address to use the app

Public addresses and extended public keys can reveal transaction history, so they still deserve care. BitGarth uses internal IDs in URLs so addresses and xpubs are not exposed through browser history, cached pages, or proxy logs.

Public wallet data cannot spend funds. BitGarth treats it as sensitive financial data without turning it into custody risk.

Future exchange support will use API keys instead of exchange login passwords. Those API keys will be saved in your encrypted user database, like the Etherscan API key BitGarth can store today.

## Encrypted User Storage

Your private BitGarth data is stored in a SQLCipher-encrypted SQLite user database.

Bitcoin history proof and sync-repair information is stored there too, beside
the wallet history it verifies. It is not stored in BitGarth's unencrypted app
database.

The user database is protected by a randomly generated 256-bit data encryption key. Your password is used to derive a key with Argon2id. That derived key wraps the database encryption key, so changing your password can re-wrap the key without rewriting the entire database.

Your password unlocks the user database. BitGarth cannot recover it for you. If you lose the password and do not have an export or backup, you will need to re-add and re-sync your wallets. Encrypted exports and backups are there to make that avoidable.

## Paired CLI Access

You can pair the BitGarth command-line client with an app account to read wallet
balances. Pairing creates a Client Key: a second way to unlock the same
encrypted user database without entering your password for every command. Your
private data remains in that encrypted database.

The CLI requires HTTPS by default and validates server certificates. It does
not follow redirects. Plain HTTP works only when you pass
`--allow-insecure-http` or type an explicit `yes` at the interactive risk
prompt.

Your local CLI profile contains the Client Key. Treat the profile file as a
high-value secret: keep it under your own OS account, do not copy it into shell
history or support reports, and do not sync it to an untrusted service. Anyone
who obtains an active Client Key can use its approved permission until you
revoke it or it expires.

## App Account Records

BitGarth also has an app database for login records and app-level metadata. This app database is not encrypted.

The app database can contain your app username, login metadata, and the fact that your app account acknowledged a specific version of the Terms and Privacy Notice. It should not contain wallet addresses, transaction history, balances, labels, saved API keys, invoice details, or other private financial records.

If you run BitGarth yourself, this app database stays with your app instance and is not sent to BitGarth. If you use a BitGarth-hosted app in the future, BitGarth may operate the app database for that hosted service and keep the minimum records needed to run it.

## What BitGarth Knows About You

BitGarth does not need your name, address, or email address to use the app.

For paid plans, BitGarth uses pseudonymous payment and subscription identifiers that are separate from your local app user ID. These let the app retrieve payment and plan status without requiring a separate account on BitGarth's servers. Requests also expose normal network metadata, so they should not be understood as anonymous.

Cryptocurrency payments have different privacy properties depending on the asset and network used. If you want the strongest payment privacy among supported options, you can pay with Monero.

If BitGarth adds invoice support, invoice details will be entered inside the app. You will choose whether to save them, and saved invoice details will live in your encrypted user database rather than in a central BitGarth account.

## What BitGarth Does Not Protect Against

BitGarth cannot protect you from every threat.

It does not protect against:

- Malware or screen-recording software on your computer
- A compromised browser, operating system, Docker host, desktop app runtime, or mobile device
- Malicious browser extensions reading pages you open
- Someone who knows your BitGarth username/password combination and has access to your computer or files
- Losing your password without a backup
- Publishing your exported accounting files somewhere public
- Blockchain privacy limits inherent to public addresses and xpubs

If your device or host is compromised, assume your financial data can be compromised too. BitGarth's model is local-first, not invincible.

## Exports Are Yours

BitGarth exports to plain-text accounting formats such as hledger and ledger-cli. That is intentional.

Plain text is easy to inspect, back up, diff, archive, and move between tools. It is also a natural fit for AI analysis.

That openness means exported files are your responsibility. Treat them like financial records. Store them somewhere you control, encrypt backups where appropriate, and avoid syncing them into services you do not trust.

If an account's Bitcoin history is still syncing, unscanned, or stopped by its
configured transaction limit, hledger exports keep the transaction postings
and any persisted transaction/provider balance assertions for the available
history window. They leave out only the separate year-opening and year-closing
journals that require complete history. Once history is complete, those yearly
boundary journals are included normally.

## Network Requests

BitGarth connects to blockchain data providers such as Mempool and Etherscan so it can sync public transaction history for the addresses and xpubs you add. Those providers can see the public wallet data being queried.

BitGarth can also fetch market prices from CoinGecko. This is optional and off by default. When you enable it, BitGarth sends CoinGecko asset IDs and your selected currency, such as `bitcoin` and `USD`. It does not send wallet addresses, xpubs, account labels, your app username, or transaction history for price lookups.

BitGarth may cache fetched market prices in a local unencrypted `prices.db` so it does not need to ask CoinGecko for the same current price repeatedly. That cache stores public market data such as asset ID, selected currency, CoinGecko ID, price, and retrieval time. It does not store wallet addresses, xpubs, balances, account labels, saved API keys, or your app username.

Manual asset discovery can also use CoinGecko. This is optional. BitGarth first searches its built-in catalog and locally cached public CoinGecko catalog data. If you choose to search CoinGecko or look up a CoinGecko-only asset, BitGarth sends the selected CoinGecko asset ID, not your wallet addresses, xpubs, balances, labels, app username, or transaction history.

The shared `prices.db` may cache public CoinGecko discovery metadata such as asset IDs, symbols, names, platform IDs, and public contract references. Your manual account choices, labels, balances, and confirmed precision stay in your encrypted user database.

Where possible, BitGarth is designed so you can choose or self-host the services it talks to, for instance your own hosted Mempool instance.

The goal is not to pretend public-chain privacy is solved. The goal is to give you clear control over where your data goes.

## Requests to BitGarth's Servers

Your app contacts BitGarth's servers for software updates, product and plan information, and payment and subscription services. Product and plan refreshes also run at login, including free-tier logins. Disabling automatic software update checks does not disable login refreshes, payment requests, or checks you explicitly request.

These requests carry technical information about the app in BitGarth-specific headers:

- `X-BitGarth-App-Version`: the app version, which can include a build revision, such as `0.3.1-a1b2c3d`.
- `X-BitGarth-App-Channel`: the distribution channel or build type, such as `docker`.
- Builds that provide platform information also send `X-BitGarth-App-Platform`: the operating system and processor architecture the app binary was built for, such as `linux/x86_64`.

Platform information describes the binary making the request, not the device you use to browse a remotely hosted app. A Linux container on a Mac reports Linux; a binary running under architecture emulation reports its compiled architecture. A distribution-channel label is supplied by the person packaging or configuring the app.

We may use this information to diagnose compatibility problems, understand which builds and platforms contact our services, and guide platform support. These fields accompany existing requests; they do not create additional reporting requests or report screen views or recent CLI activity. These BitGarth-specific headers are sent to BitGarth's servers, not to blockchain or market-data providers.

BitGarth does not derive these fields from wallet data or your local account username, and does not generate an installation or device identifier for this reporting. BitGarth's servers also receive normal network metadata such as IP address, request time, requested service, response status, and User-Agent. On payment and subscription requests, build information may be associated with the pseudonymous payment or subscription identifiers carried by that request. Those requests also carry the credentials needed for the payment service; the limits on build metadata do not mean that the entire request contains only build information.

See the [Privacy Notice](https://bitgarth.app/privacy.html) for how this request metadata is used and retained.

## Responsible Disclosure


See [SECURITY.md](../../SECURITY.md) for the repository's reporting policy.

## The Principle

BitGarth is built on a narrow promise: your self-custody accounting should not require surrendering custody of your data.

Encrypted user storage, public-key-only wallet access, plain-text exports, and honest limits all serve that promise.

## Local Startup Reports

If BitGarth cannot initialize its app database, it prints a diagnostic and,
when possible, saves `startup-error.txt` in the project directory. The report
contains the failure timestamp, build identifier, startup stage and category,
filesystem paths, migration identifiers and checksums when relevant, and
filesystem or SQLite error codes. It does not include wallet records,
passwords, session tokens, keys, or envelopes, and is not uploaded to
BitGarth's servers.

The report is limited to 16 KiB. A later failed start replaces it; a successful
start leaves it as historical evidence. Paths may include your OS username, so
review and redact the report before sharing it. See
[Startup troubleshooting](startup-troubleshooting.md) for safe recovery.
