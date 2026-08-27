import { MediaType, RetryPolicies, SymbolClient } from "/symbol.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });
const NUMBER_WIDTH = 10;
const ANNOTATION_MEDIA = MediaType.application("vnd.symbol.annotation");

const annotationsElement = document.querySelector("#annotations");
const template = document.querySelector("#annotation-template");
const statusElement = document.querySelector("#status");
const newButton = document.querySelector("#new-button");
const composer = document.querySelector("#composer");
const composerMode = document.querySelector("#composer-mode");
const selectionPreview = document.querySelector("#selection-preview");
const commentInput = document.querySelector("#comment");
const saveButton = document.querySelector("#save-button");
const cancelButton = document.querySelector("#cancel-button");
const settingsButton = document.querySelector("#settings-button");
const settings = document.querySelector("#settings");
const tokenInput = document.querySelector("#token");
const applyToken = document.querySelector("#apply-token");
const siteLabel = document.querySelector("#site-label");

const siteName = location.pathname.split("/").filter(Boolean)[0];
if (!siteName) {
  throw new Error("Host this demo as a named Symbol site, not at the origin root.");
}

let token = sessionStorage.getItem("symbol-demo-token") || "";
let symbol;
let site;
let annotations = [];
let selectedRange = null;
let editing = null;

connect();
siteLabel.textContent = `/${siteName}`;
tokenInput.value = token;

document.addEventListener("selectionchange", captureSelection);
newButton.addEventListener("click", beginNewAnnotation);
cancelButton.addEventListener("click", closeComposer);
saveButton.addEventListener("click", saveAnnotation);
settingsButton.addEventListener("click", () => settings.showModal());
applyToken.addEventListener("click", () => {
  token = tokenInput.value.trim();
  if (token) {
    sessionStorage.setItem("symbol-demo-token", token);
  } else {
    sessionStorage.removeItem("symbol-demo-token");
  }
  connect();
  void loadAnnotations();
});

await loadAnnotations();

function connect() {
  symbol = new SymbolClient({
    origin: location.origin,
    token: token || undefined,
    retryPolicy: RetryPolicies.Default,
  });
  site = symbol.site(siteName);
}

function captureSelection() {
  if (!composer.hidden) return;
  const selection = getSelection();
  if (selection?.rangeCount !== 1 || selection.isCollapsed) {
    selectedRange = null;
    newButton.disabled = true;
    return;
  }

  const range = selection.getRangeAt(0);
  const block = containingBlock(range.startContainer);
  if (!block || block !== containingBlock(range.endContainer)) {
    selectedRange = null;
    newButton.disabled = true;
    return;
  }

  const start = textOffset(block, range.startContainer, range.startOffset);
  const end = textOffset(block, range.endContainer, range.endOffset);
  const quote = block.textContent.slice(start, end);
  if (!quote.trim()) {
    selectedRange = null;
    newButton.disabled = true;
    return;
  }

  selectedRange = {
    block: block.dataset.block,
    start,
    end,
    quote,
  };
  newButton.disabled = false;
}

function containingBlock(node) {
  const element = node.nodeType === Node.ELEMENT_NODE ? node : node.parentElement;
  return element?.closest("[data-block]") || null;
}

function textOffset(block, node, offset) {
  const prefix = document.createRange();
  prefix.selectNodeContents(block);
  prefix.setEnd(node, offset);
  return prefix.toString().length;
}

function beginNewAnnotation() {
  if (!selectedRange) return;
  editing = null;
  composerMode.textContent = "New annotation";
  selectionPreview.textContent = selectedRange.quote;
  commentInput.value = "";
  composer.hidden = false;
  commentInput.focus();
}

function beginEdit(annotation) {
  editing = annotation;
  selectedRange = null;
  newButton.disabled = true;
  composerMode.textContent = `Editing ${annotation.path.split("/").at(-1)}`;
  selectionPreview.textContent = annotation.quote;
  commentInput.value = annotation.comment;
  composer.hidden = false;
  commentInput.focus();
}

function closeComposer() {
  composer.hidden = true;
  editing = null;
  commentInput.value = "";
}

async function saveAnnotation() {
  const comment = commentInput.value.trim();
  if (!comment) {
    commentInput.focus();
    return;
  }

  saveButton.disabled = true;
  setStatus(editing ? "Saving byte splice…" : "Allocating annotation file…");
  try {
    if (editing) {
      await editAnnotation(editing, comment);
    } else if (selectedRange) {
      await createAnnotation(selectedRange, comment);
      getSelection()?.removeAllRanges();
      selectedRange = null;
      newButton.disabled = true;
    }
    closeComposer();
    await loadAnnotations();
  } catch (error) {
    setStatus(formatError(error), true);
  } finally {
    saveButton.disabled = false;
  }
}

async function createAnnotation(selection, comment) {
  const payload = encodeAnnotation({
    block: selection.block,
    start: selection.start,
    end: selection.end,
    created: new Date().toISOString(),
    comment,
  });
  const receipt = await site.folder("annotations").bytes(payload, {
    mediaType: ANNOTATION_MEDIA,
    name: { prefix: "note-", extension: "anno" },
  });
  setStatus(`Created ${receipt.path}`);
}

async function editAnnotation(annotation, comment) {
  const commentBytes = encoder.encode(comment);
  const lengthBytes = encoder.encode(fixedNumber(commentBytes.byteLength));
  const file = site.file(annotation.path);
  const baseHash = await file.hash();

  await file.patch(
    [
      {
        offset: annotation.commentLengthOffset,
        deleteBytes: NUMBER_WIDTH,
        insert: lengthBytes,
      },
      {
        offset: annotation.commentOffset,
        deleteBytes: annotation.commentByteLength,
        insert: commentBytes,
      },
    ],
    { baseHash },
  );
  setStatus(`Patched ${annotation.path}`);
}

async function loadAnnotations() {
  setStatus("Loading annotations…");
  try {
    const inventory = await site.files();
    if (inventory.status !== 200) {
      throw new Error("The annotation inventory returned no body.");
    }

    const paths = inventory.files
      .map((file) => file.path)
      .filter((path) => path.startsWith("annotations/") && path.endsWith(".anno"));
    annotations = (await Promise.all(paths.map((path) => readAnnotation(path)))).sort(
      compareAnnotations,
    );
    renderAnnotations();
    setStatus(
      annotations.length === 0
        ? "Select a sentence to make the first annotation."
        : `${annotations.length} annotation${annotations.length === 1 ? "" : "s"}`,
    );
  } catch (error) {
    annotations = [];
    renderAnnotations();
    setStatus(formatError(error), true);
  }
}

async function readAnnotation(path) {
  const response = await site.file(path).get();
  try {
    if (response.status !== 200) {
      throw new Error(`Could not read ${path}: HTTP ${response.status}`);
    }
    return decodeAnnotation(new Uint8Array(await response.arrayBuffer()), path);
  } finally {
    await response[Symbol.asyncDispose]();
  }
}

function encodeAnnotation(annotation) {
  const comment = encoder.encode(annotation.comment);
  const beforeLength = [
    "SYMBOL-ANNOTATION/1",
    `block:${encodeURIComponent(annotation.block)}`,
    `start:${fixedNumber(annotation.start)}`,
    `end:${fixedNumber(annotation.end)}`,
    `created:${annotation.created}`,
    "comment-bytes:",
  ].join("\n");
  const header = encoder.encode(`${beforeLength}${fixedNumber(comment.byteLength)}\n\n`);
  const payload = new Uint8Array(header.byteLength + comment.byteLength);
  payload.set(header);
  payload.set(comment, header.byteLength);
  return payload;
}

function decodeAnnotation(bytes, path) {
  const separator = findBytes(bytes, new Uint8Array([10, 10]));
  if (separator < 0) throw new Error(`${path} has no annotation header terminator.`);

  const header = decoder.decode(bytes.slice(0, separator));
  const lines = header.split("\n");
  if (lines.shift() !== "SYMBOL-ANNOTATION/1") {
    throw new Error(`${path} uses an unsupported annotation format.`);
  }

  const fields = new Map(
    lines.map((line) => {
      const split = line.indexOf(":");
      if (split < 1) throw new Error(`${path} has a malformed annotation field.`);
      return [line.slice(0, split), line.slice(split + 1)];
    }),
  );
  const block = decodeURIComponent(requiredField(fields, "block", path));
  const start = strictNumber(requiredField(fields, "start", path), path);
  const end = strictNumber(requiredField(fields, "end", path), path);
  const created = requiredField(fields, "created", path);
  const commentByteLength = strictNumber(requiredField(fields, "comment-bytes", path), path);
  const commentOffset = separator + 2;
  const commentBytes = bytes.slice(commentOffset);
  if (commentBytes.byteLength !== commentByteLength) {
    throw new Error(`${path} has an inconsistent comment length.`);
  }

  const marker = encoder.encode("comment-bytes:");
  const markerOffset = findBytes(bytes.slice(0, separator), marker);
  if (markerOffset < 0) throw new Error(`${path} omitted comment-bytes.`);

  const blockElement = document.querySelector(`[data-block="${CSS.escape(block)}"]`);
  const quote = blockElement
    ? blockElement.textContent.slice(start, end)
    : "Referenced text is not in this document.";

  return {
    path,
    block,
    start,
    end,
    created,
    comment: decoder.decode(commentBytes),
    commentOffset,
    commentByteLength,
    commentLengthOffset: markerOffset + marker.byteLength,
    quote,
  };
}

function requiredField(fields, name, path) {
  const value = fields.get(name);
  if (value === undefined) throw new Error(`${path} omitted ${name}.`);
  return value;
}

function fixedNumber(value) {
  if (!Number.isSafeInteger(value) || value < 0 || value >= 10 ** NUMBER_WIDTH) {
    throw new RangeError(`Annotation number is outside 0–${10 ** NUMBER_WIDTH - 1}.`);
  }
  return String(value).padStart(NUMBER_WIDTH, "0");
}

function strictNumber(value, path) {
  if (!new RegExp(`^[0-9]{${NUMBER_WIDTH}}$`).test(value)) {
    throw new Error(`${path} contains a malformed fixed-width number.`);
  }
  return Number(value);
}

function findBytes(haystack, needle) {
  outer: for (let at = 0; at <= haystack.length - needle.length; at += 1) {
    for (let index = 0; index < needle.length; index += 1) {
      if (haystack[at + index] !== needle[index]) continue outer;
    }
    return at;
  }
  return -1;
}

function compareAnnotations(left, right) {
  const blocks = Array.from(document.querySelectorAll("[data-block]")).map(
    (block) => block.dataset.block,
  );
  return (
    blocks.indexOf(left.block) - blocks.indexOf(right.block) ||
    left.start - right.start ||
    left.path.localeCompare(right.path)
  );
}

function renderAnnotations() {
  annotationsElement.replaceChildren();
  const ranges = [];

  annotations.forEach((annotation, index) => {
    const card = template.content.firstElementChild.cloneNode(true);
    card.querySelector(".annotation-number").textContent = String(index + 1).padStart(2, "0");
    card.querySelector(".annotation-quote").textContent = annotation.quote;
    card.querySelector(".annotation-comment").textContent = annotation.comment;
    card.querySelector(".annotation-path").textContent = annotation.path.split("/").at(-1);
    card.querySelector(".edit-button").addEventListener("click", () => beginEdit(annotation));
    card.querySelector(".annotation-target").addEventListener("click", () => {
      const block = document.querySelector(`[data-block="${CSS.escape(annotation.block)}"]`);
      block?.scrollIntoView({ behavior: "smooth", block: "center" });
    });
    annotationsElement.append(card);

    const block = document.querySelector(`[data-block="${CSS.escape(annotation.block)}"]`);
    const range = block ? rangeForOffsets(block, annotation.start, annotation.end) : null;
    if (range) ranges.push(range);
  });

  if (CSS.highlights) {
    CSS.highlights.set("symbol-annotations", new Highlight(...ranges));
  }
}

function rangeForOffsets(root, start, end) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  let position = 0;
  let startSet = false;
  while (walker.nextNode()) {
    const node = walker.currentNode;
    const next = position + node.data.length;
    if (!startSet && start >= position && start <= next) {
      range.setStart(node, start - position);
      startSet = true;
    }
    if (startSet && end >= position && end <= next) {
      range.setEnd(node, end - position);
      return range;
    }
    position = next;
  }
  return null;
}

function setStatus(message, error = false) {
  statusElement.textContent = message;
  statusElement.style.color = error ? "var(--pencil)" : "";
}

function formatError(error) {
  if (error && typeof error === "object" && "message" in error) {
    return error.message;
  }
  return String(error);
}
