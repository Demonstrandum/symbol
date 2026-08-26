# symbol

tailnet hosting of static sites on {host}.
an HTML file, a folder of pages and assets, or an archive.
archives (zip, tar, tar.gz, gz) can be unpacked with `-H unpack:1`.

GET is the site. PUT adds files. DELETE pops a site as tar.gz.

see [{host}/FILES]({host}/FILES) for site listing.

## the client

for convenience, you may install the utility client.
drops `symbol` in ~/.local/bin

```
curl -fsSL {host}/install.sh | sh
```

then

```
symbol put index.html        # upload html, random name
symbol put hello index.html  # upload html to {host}/hello
symbol put hello ./dist      # merge a folder into {host}/hello
symbol put -u hello site.zip # merge an unpacked zip
symbol put hello style.css   # add or update one file
symbol clone hello           # make a local checkout
symbol copy hello hello-copy # duplicate a site on the server
symbol remix hello           # duplicate and clone locally
symbol move hello-copy moved # rename without copying files
symbol sync                  # publish if upstream has not changed
symbol ls                    # list all sites
symbol stats                 # storage totals and distributions
symbol get hello             # download without removing
symbol pop hello             # download and remove
symbol rm hello              # remove without backup
symbol undo --stack hello    # changes that can be undone
symbol expire                # expiry help and retention graph
symbol manage hello --status # show write protection
symbol recover               # resume interrupted creations
symbol update                # reinstall this client
```

uploading to an existing site only adds or updates the files you send; it does
not remove anything else. every clone contains a generated `symbol.toml`, so
running bare `symbol put` from that directory publishes it to the same site.
`symbol sync` is stricter and refuses if upstream has changed.

copy and move refuse an existing destination. `symbol undo` reverses the most
recent retained mutation. managed sites use `symbol manage NAME --claim`,
`--rotate`, or `--release`; tokens are read from `-t`, `SYMBOL_TOKEN`, then
`symbol.toml`.
the server redacts recognizable Symbol tokens from inspectable uploads, but
encrypted or otherwise opaque content cannot be guaranteed safe.

`symbol.toml` records the site name, host, revision, tree hash, and file
baseline. sync sends that tree hash as `If-Match`; upstream drift returns
`412` without writing. expiry supports `--in`, `--at`, `--decay`, and
`--never`. generated PUT/COPY requests use idempotency keys for safe retry.
copy and move return `409` when their destination exists; use the printed
`symbol undo` command to reverse a retained mutation.

creator identity may come from a trusted proxy account, an mTLS fingerprint
supplied by a trusted TLS terminator, a Tailscale user, or a mode-0600 creator
claim; Symbol does not terminate TLS itself. PUT adds or updates paths;
`rm NAME PATH` removes a path, `pop` removes a whole site after downloading it,
copy creates a new site, and move changes its name. each mutation prints its
inverse `symbol undo` command while retained.

## full API manuals

- [JavaScript and TypeScript]({host}/API/JS)
- [Python 3.14]({host}/API/PY)
- [shell client]({host}/API/SH)
- [curl and HTTP protocol]({host}/API/CURL)

the built-in [{host}/API/]({host}/API/) site is compiled from the canonical
API reference and cannot be modified like a hosted site.
