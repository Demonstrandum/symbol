# Margin annotation demo

This is a static site that uses the Symbol instance hosting it as its database.
It has no build step and no dependencies.

```sh
symbol put annotation-demo examples/annotation-site
```

Open `/annotation-demo/`, select text within one paragraph, and save a comment.

- New annotations use `FolderClient.bytes()` and receive server-assigned names
  such as `annotations/note-<blake3>.anno`.
- Existing comments use `FileClient.patch()` with two byte splices: one updates
  the fixed-width comment length and one replaces the UTF-8 comment body.
- Annotation files live on the same site under `annotations/`.
- Managed sites can enter a token through the **Access** dialog; it is kept only
  in `sessionStorage`.

The `.anno` demo format is intentionally small:

```text
SYMBOL-ANNOTATION/1
block:storage
start:0000000010
end:0000000032
created:2026-08-27T12:00:00.000Z
comment-bytes:0000000018

UTF-8 comment body
```
