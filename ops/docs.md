# symbol

tailnet hosting of static sites on {host}.
an HTML file, a folder of pages and assets, or an archive.
archives (zip, tar, tar.gz, gz) can be unpacked with `-H unpack:1`.

GET is the site. PUT writes it. DELETE pops a site as tar.gz.

## the client

for conveience, you may install the utility client.
drops `symbol` in ~/.local/bin

```
curl -fsSL {host}/install.sh | sh
```

then

```
symbol put index.html        # upload html, random name
symbol put hello index.html  # upload html to {host}/hello
symbol put hello ./dist      # upload whole folder to {host}/hello
symbol put -u hello site.zip # upload and unzip contents of zip
symbol add hello style.css   # add a file to the site {host}/hello
symbol ls          # list all sites
symbol pop hello  # removes a site and downloads its tar.gz
symbol rm hello   # removes a site without backup
symbol update     # reinstall this client
```

## curl

no name provided gets you a a 4-character id, e.g. {host}/k7qm/

```
curl -T index.html {host}/
curl -T index.html {host}/hello
```

add a file to that site: PUT

```
curl -T style.css {host}/hello/style.css
curl -T style.css {host}/hello/  # same; curl appends the local filename
```

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

file browse: GET {host}/FILES lists sites. GET {host}/path/HASH is that file's hash.

```
curl {host}/FILES
curl {host}/hello/FILES
curl {host}/hello/FILES/css/
curl {host}/hello/index.html/HASH
```

delete a file, or pop a site (DELETE returns the site as tar.gz)

```
curl -X DELETE {host}/hello/style.css
curl -OJ -X DELETE {host}/hello  # removes, saves locally as hello.tar.gz
```
