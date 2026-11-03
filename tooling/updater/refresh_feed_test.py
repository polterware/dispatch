import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("refresh_feed", Path(__file__).with_name("refresh-feed.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class FeedTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        root = Path(self.directory.name)
        self.target = root / "dispatch.xml"
        self.target.write_text('<rss><channel><title>Dispatch</title></channel></rss>')
        self.incoming = root / "appcast.xml"
        self.tag = "dispatch-v0.1.0-alpha.4"
        self.xml = '''<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>
<sparkle:version>200</sparkle:version><sparkle:shortVersionString>0.1.0-alpha.4</sparkle:shortVersionString>
<enclosure url="https://github.com/polterware/dispatch/releases/download/dispatch-v0.1.0-alpha.4/Dispatch-0.1.0-alpha.4-arm64.zip"
length="42" sparkle:edSignature="''' + "A" * 86 + '''==" /></item></channel></rss>'''

    def advance(self, xml=None):
        self.incoming.write_text(xml or self.xml)
        module.advance("dispatch", self.tag, self.incoming, self.target)

    def test_advances_only_the_target_feed(self):
        other = self.target.with_name("other.xml")
        other.write_text("unchanged")
        self.advance()
        self.assertEqual(self.target.read_text(), self.xml)
        self.assertEqual(other.read_text(), "unchanged")

    def test_rejects_external_archive_urls(self):
        with self.assertRaises(ValueError):
            self.advance(self.xml.replace("github.com/polterware", "example.com/polterware"))

    def test_rejects_mismatched_release_versions(self):
        with self.assertRaises(ValueError):
            self.advance(self.xml.replace("<sparkle:shortVersionString>0.1.0-alpha.4", "<sparkle:shortVersionString>0.1.0-alpha.5"))

    def test_rejects_missing_signature(self):
        with self.assertRaises(ValueError):
            self.advance(self.xml.replace("A" * 86 + "==", ""))

    def test_does_not_roll_back_a_published_feed(self):
        self.advance()
        self.advance(self.xml.replace(">200<", ">100<"))
        self.assertEqual(self.target.read_text(), self.xml)
