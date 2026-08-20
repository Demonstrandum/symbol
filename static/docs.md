# symbol

tailnet hosting of static sites on {host}.
an HTML file, a folder of pages and assets, or an archive.
archives (zip, tar, tar.gz, gz) can be unpacked with `-H unpack:1`.

GET is the site. PUT adds files. DELETE pops a site as tar.gz.

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
symbol sync                  # publish if upstream has not changed
symbol ls                    # list all sites
symbol stats                 # storage totals and distributions
symbol get hello             # download without removing
symbol pop hello             # download and remove
symbol rm hello              # remove without backup
symbol undo --stack hello    # changes that can be undone
symbol expire                # expiry help and retention graph
symbol update                # reinstall this client
```

uploading to an existing site only adds or updates the files you send; it does
not remove anything else. every clone contains a generated `symbol.toml`, so
running bare `symbol put` from that directory publishes it to the same site.
`symbol sync` is stricter and refuses if upstream has changed.

## curl

no name provided gets you a 4-character id, e.g. [{host}/k7qm/]({host}/k7qm/)

```
curl -T index.html {host}/
curl -T index.html {host}/hello
```

add a file to that site: PUT

```
curl -T style.css {host}/hello/style.css
curl -T style.css {host}/hello/  # same; curl appends the local filename
```

large stored files are uploaded and served as streams. byte ranges, seeking,
resume, HEAD, and browser buffering are supported for media such as mp3/mp4.

```
curl -T movie.mp4 {host}/media/movie.mp4
curl -H 'Range: bytes=1000000-' {host}/media/movie.mp4
```

individual stored files can be up to 4 GiB. archive unpacking is limited to
50 MiB compressed and 80 MiB extracted.

a zip, stored as a file

```
curl -T site.zip {host}/hello
```

unpack a zip / tar / tar.gz / gz into the site

```
curl -T site.zip -H unpack:1 {host}/hello
curl -T site.tar.gz -H unpack:1 {host}/hello
curl -T notes.txt.gz -H unpack:1 {host}/hello
```

a directory, via tar (always unpacked)

```
tar -czf - -C ./dist . | curl -T -    \
  -H 'Content-Type: application/gzip' \
  -H unpack:1 {host}/hello
```

file browse: GET {host}/FILES lists sites. site and folder sizes are recursive
logical sizes, so duplicate content is counted once per file reference.
GET {host}/path/HASH is that file's hash.

```
curl {host}/FILES
curl {host}/hello/FILES
curl {host}/hello/FILES/css/
curl -H 'Accept: application/json' {host}/hello/FILES
curl {host}/hello/index.html/HASH
```

copy, move, undo, and expiry are HTTP methods too

```
curl -X COPY {host}/hello -H 'Destination: /hello-copy'
curl -X MOVE {host}/hello-copy -H 'Destination: /hello-moved'
curl {host}/hello/UNDO
curl -X UNDO {host}/hello
curl -X EXPIRE {host}/hello
```

download a site without removing it

```
curl -OJ {host}/hello.tar.gz
curl -OJ {host}/hello.tar
curl -OJ {host}/hello.zip
```

delete a file, or pop a site (DELETE returns the site as tar.gz)

```
curl -X DELETE {host}/hello/style.css
curl -OJ -X DELETE {host}/hello  # removes, saves locally as hello.tar.gz
```
