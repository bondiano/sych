#!/usr/bin/env python3
"""Scripted LSP server for semantic adapter tests.

Answers from the materialized workspace (its cwd): every `function NAME(` line opens a
symbol that ends at the next line starting with `}`; references and incoming calls are
lines containing `NAME(`; outgoing calls are the known functions called inside the symbol;
a definition is the `interface/type/class` of the word at the position. A `let NAME` line
is a variable; its references are the other occurrences of the word, and `documentHighlight`
marks `NAME =` as a write and any other occurrence as a read.
Capabilities come from `caps.json` next to this script, so a
test can withdraw call hierarchy or references. Positions are UTF-16, as LSP defaults.
"""
import json
import os
import re
import sys
from urllib.parse import unquote, urlparse

HERE = os.path.dirname(os.path.abspath(__file__))
CAPS = json.load(open(os.path.join(HERE, "caps.json")))
ROOT = os.getcwd()
FUNCTION = re.compile(r"function (\w+)\(")
VARIABLE = re.compile(r"let (\w+)")
TYPE = re.compile(r"(?:interface|type|class) (\w+)")


def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.decode().strip()
        if not line:
            break
        if line.lower().startswith("content-length:"):
            length = int(line.split(":")[1])
    return json.loads(sys.stdin.buffer.read(length))


def send(message):
    body = json.dumps(message).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()


def path_of(uri):
    return unquote(urlparse(uri).path)


def uri_of(path):
    return "file://" + path


def utf16(text):
    return len(text.encode("utf-16-le")) // 2


def symbols(path):
    lines = open(path, encoding="utf-8").read().split("\n")
    out = []
    for i, line in enumerate(lines):
        variable = VARIABLE.search(line)
        if variable:
            col = utf16(line[: variable.start(1)])
            out.append({
                "name": variable.group(1),
                "kind": 13,
                "range": {"start": {"line": i, "character": 0},
                          "end": {"line": i, "character": utf16(line)}},
                "selectionRange": {"start": {"line": i, "character": col},
                                   "end": {"line": i, "character": col + utf16(variable.group(1))}},
            })
            continue
        match = FUNCTION.search(line)
        if not match:
            continue
        end = next((j for j in range(i + 1, len(lines)) if lines[j].startswith("}")), i)
        start_col = utf16(line[: match.start(1)])
        out.append({
            "name": match.group(1),
            "kind": 12,
            "range": {"start": {"line": i, "character": 0},
                      "end": {"line": end, "character": utf16(lines[end])}},
            "selectionRange": {"start": {"line": i, "character": start_col},
                               "end": {"line": i, "character": start_col + utf16(match.group(1))}},
        })
    return out


def files():
    for dirpath, _, names in os.walk(ROOT):
        for name in sorted(names):
            if name.endswith((".ts", ".tsx")):
                yield os.path.join(dirpath, name)


def name_at(uri, position):
    for symbol in symbols(path_of(uri)):
        if symbol["selectionRange"]["start"]["line"] == position["line"]:
            return symbol
    return None


def uses(name):
    """(path, line index, enclosing symbol) of every `name(` that is not its definition."""
    for path in files():
        lines = open(path, encoding="utf-8").read().split("\n")
        enclosing = symbols(path)
        for i, line in enumerate(lines):
            if name + "(" in line and "function " + name not in line:
                caller = next((s for s in enclosing
                               if s["range"]["start"]["line"] <= i <= s["range"]["end"]["line"]), None)
                col = utf16(line[: line.index(name)])
                yield path, i, col, caller


def occurrences(path, name):
    """(line index, column, is write) of every whole-word `name` outside its `let`."""
    for i, line in enumerate(open(path, encoding="utf-8").read().split("\n")):
        for match in re.finditer(r"\b%s\b" % re.escape(name), line):
            if line[: match.start()].endswith("let "):
                continue
            write = re.match(r"\s*=(?!=)", line[match.end():]) is not None
            yield i, utf16(line[: match.start()]), write


def location(path, line, col, width):
    return {"uri": uri_of(path),
            "range": {"start": {"line": line, "character": col},
                      "end": {"line": line, "character": col + width}}}


def word_at(uri, position):
    line = open(path_of(uri), encoding="utf-8").read().split("\n")[position["line"]]
    units = 0
    for i, ch in enumerate(line):
        if units >= position["character"]:
            return re.match(r"\w+", line[i:]).group(0)
        units += utf16(ch)
    return None


def type_definition(name):
    """A LocationLink to `interface/type/class NAME`, its range up to the closing `}`."""
    for path in files():
        lines = open(path, encoding="utf-8").read().split("\n")
        for i, line in enumerate(lines):
            match = TYPE.search(line)
            if match and match.group(1) == name:
                end = next((j for j in range(i, len(lines)) if lines[j].startswith("}")), i)
                full = {"start": {"line": i, "character": 0},
                        "end": {"line": end, "character": utf16(lines[end])}}
                return [{"targetUri": uri_of(path), "targetRange": full, "targetSelectionRange": full}]
    return None


def answer(method, params):
    if method == "initialize":
        return {"capabilities": CAPS, "serverInfo": {"name": "fake-lsp", "version": "1.0"}}
    if method == "textDocument/documentSymbol":
        return symbols(path_of(params["textDocument"]["uri"]))
    if method == "textDocument/references":
        symbol = name_at(params["textDocument"]["uri"], params["position"])
        if symbol is None:
            return []
        name = symbol["name"]
        if symbol["kind"] == 13:
            return [location(p, i, c, utf16(name)) for p in files() for i, c, _ in occurrences(p, name)]
        return [location(p, i, c, utf16(name)) for p, i, c, _ in uses(name)]
    if method == "textDocument/documentHighlight":
        path = path_of(params["textDocument"]["uri"])
        name = word_at(params["textDocument"]["uri"], params["position"])
        return [dict(location(path, i, c, utf16(name)), kind=3 if write else 2)
                for i, c, write in occurrences(path, name)]
    if method == "textDocument/definition":
        return type_definition(word_at(params["textDocument"]["uri"], params["position"]))
    if method == "textDocument/implementation":
        return None
    if method == "textDocument/prepareCallHierarchy":
        symbol = name_at(params["textDocument"]["uri"], params["position"])
        if symbol is None:
            return None
        return [dict(symbol, uri=params["textDocument"]["uri"])]
    if method == "callHierarchy/incomingCalls":
        name = params["item"]["name"]
        calls = []
        for path, i, col, caller in uses(name):
            if caller is None:
                continue
            calls.append({"from": dict(caller, uri=uri_of(path)),
                          "fromRanges": [location(path, i, col, utf16(name))["range"]]})
        return calls
    if method == "callHierarchy/outgoingCalls":
        item = params["item"]
        lines = open(path_of(item["uri"]), encoding="utf-8").read().split("\n")
        body = lines[item["range"]["start"]["line"] + 1: item["range"]["end"]["line"] + 1]
        return [{"to": dict(s, uri=uri_of(p)), "fromRanges": []}
                for p in files() for s in symbols(p)
                if any(s["name"] + "(" in line for line in body)]
    if method == "textDocument/diagnostic":
        return {"kind": "full", "items": []}
    return None


while True:
    message = read()
    method = message.get("method")
    if method == "exit":
        sys.exit(0)
    if "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "result": answer(method, message.get("params", {}))})
