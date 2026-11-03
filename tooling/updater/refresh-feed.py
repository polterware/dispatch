"""Validate a published release appcast before advancing its app-specific feed."""
import base64
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
PRODUCTS = {"dispatch": "Dispatch"}
REPOSITORY = "polterware/dispatch"


def advance(app: str, tag: str, incoming: Path, target: Path) -> None:
    if app not in PRODUCTS or not re.fullmatch(rf"{app}-v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag):
        raise ValueError("Invalid release tag")
    version = tag[len(app) + 2:]
    root = ET.fromstring(incoming.read_bytes())
    items = root.findall("./channel/item")
    if root.tag != "rss" or len(items) != 1:
        raise ValueError("Expected one signed release item")
    item = items[0]
    build = item.findtext(SPARKLE + "version", "")
    if not build.isascii() or not build.isdecimal() or int(build) <= 0:
        raise ValueError("Invalid build number")
    if item.findtext(SPARKLE + "shortVersionString") != version:
        raise ValueError("Release version does not match the feed")
    enclosure = item.find("enclosure")
    if enclosure is None:
        raise ValueError("Missing update enclosure")
    expected = f"https://github.com/{REPOSITORY}/releases/download/{tag}/{PRODUCTS[app]}-{version}-arm64.zip"
    if enclosure.get("url") != expected or int(enclosure.get("length", "0")) <= 0:
        raise ValueError("Unexpected update archive")
    if len(base64.b64decode(enclosure.get(SPARKLE + "edSignature", ""), validate=True)) != 64:
        raise ValueError("Invalid Ed25519 signature")
    previous = ET.fromstring(target.read_bytes()).findtext("./channel/item/" + SPARKLE + "version", "0")
    if int(previous) >= int(build):
        print("Feed already contains this build or a newer build")
        return
    target.write_bytes(incoming.read_bytes())
    print(f"Advanced {app} feed to {version} (build {build})")


if __name__ == "__main__":
    advance(sys.argv[1], sys.argv[2], Path(sys.argv[3]), Path(sys.argv[4]))
