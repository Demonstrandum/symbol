# symbol

tiny static web hosting on ${host}.
give it an HTML file, a folder, or an archive; get back a URL.
there are no accounts and reads are ordinary HTTP.

see [${host}/FILES](${host}/FILES) for everything currently hosted.

## with curl

upload one page to a random short URL

```
curl -T index.html ${host}/
```

choose a name, then add another file without replacing the rest of the site

```
curl -T index.html ${host}/hello
curl -T style.css ${host}/hello/style.css
```

download the complete site as an archive

```
curl -OJ ${host}/hello.tar.gz
```

GET reads, PUT adds or updates, and DELETE removes. folders and archives work
too; large files support ranges and stream without being loaded into memory.

## use the client

install the small POSIX client

```
curl -fsSL ${host}/install.sh | sh
```

the common commands

```
symbol put index.html        # upload HTML, random name
symbol put hello ./dist      # merge a folder into /hello
symbol clone hello           # make a local checkout
symbol sync                  # safely publish local changes
symbol get hello             # download without removing
symbol copy hello hello-copy # duplicate on the server
symbol remix hello           # duplicate and clone locally
symbol undo hello            # reverse the last retained change
symbol expire hello --in 30d # opt into automatic expiry
symbol manage hello --status # show write protection
symbol ls                    # list sites
```

run `symbol COMMAND --help` for focused usage.

publish a built app, clone it elsewhere, then preview and safely sync edits

```
mkdir -p dist
printf '%s\n' '<h1>first version</h1>' > dist/index.html
symbol put my-app ./dist
symbol clone my-app my-app-work
cd my-app-work
printf '%s\n' '<h1>second version</h1>' > index.html
symbol sync --check
symbol sync
```

turn a release archive into a site, then fetch the same site as a zip

```
mkdir -p release-files
printf '%s\n' '<h1>release notes</h1>' > release-files/index.html
tar -czf release.tar.gz -C release-files .
symbol put -u release release.tar.gz
symbol get release release.zip
unzip -l release.zip
```

make a branch-like copy to edit without touching the original

```
symbol remix demo demo-next
cd demo-next
printf '%s\n' '<h1>next version</h1>' > index.html
symbol sync --check
symbol sync
```

## API manuals

See [${host}/API/](${host}/API/)

* [JavaScript and TypeScript](${host}/API/JS)
* [Python](${host}/API/PY)
* [shell client](${host}/API/SH)
* [curl and HTTP protocol](${host}/API/CURL)

