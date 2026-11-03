const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFileSync } = require('node:child_process');
const config = require('./config.cjs');
// usage: node appcast.cjs <bundle directory> <version>; SOURCE_DATE_EPOCH is the build number.
const [directory, version] = process.argv.slice(2);
const { app, product } = config;
if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version ?? '')) throw Error('Invalid update version');
const build = process.env.SOURCE_DATE_EPOCH;
if (!/^\d+$/.test(build ?? '')) throw Error('Missing Sparkle build number');
const plist = path.join(directory, `${product}.app`, 'Contents/Info.plist');
const minimumSystemVersion = execFileSync('plutil', ['-extract', 'LSMinimumSystemVersion', 'raw', '-o', '-', plist], { encoding: 'utf8' }).trim();
if (!/^\d+(?:\.\d+){0,2}$/.test(minimumSystemVersion)) throw Error('Invalid minimum macOS version');
const name = `${product.replace(/ /g, '-')}-${version}-arm64.zip`;
const zip = path.join(directory, name);
const key = process.env.POLTERWARE_SPARKLE_PRIVATE_KEY;
if (key) {
  const seed = Buffer.from(key.trim(), 'base64');
  if (seed.length !== 32) throw Error('Expected a Sparkle Ed25519 seed');
  const privateKey = crypto.createPrivateKey({ key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), seed]), format: 'der', type: 'pkcs8' });
  const publicKey = crypto.createPublicKey(privateKey).export({ format: 'der', type: 'spki' }).subarray(-32).toString('base64');
  if (publicKey !== config.publicKey) throw Error('Signing key does not match the public key embedded in the apps');
}
const args = key ? ['--ed-key-file', '-', '-p', zip] : ['--account', 'polterware', '-p', zip];
const signature = execFileSync(path.join(__dirname, 'vendor/bin/sign_update'), args, { input: key ? key.trim() : undefined, encoding: 'utf8' }).trim();
if (!/^[A-Za-z0-9+/]{86}==$/.test(signature)) throw Error('Invalid Sparkle signature output');
const escape = value => String(value).replace(/[&<>"']/g, char => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;' }[char]));
const url = `https://github.com/${config.repository}/releases/download/${app}-v${version}/${name}`;
const xml = `<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>${escape(product)} updates</title>
    <item>
      <title>${escape(product)} ${escape(version)}</title>
      <sparkle:version>${build}</sparkle:version>
      <sparkle:shortVersionString>${escape(version)}</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>${minimumSystemVersion}</sparkle:minimumSystemVersion>
      <enclosure url="${escape(url)}" sparkle:edSignature="${escape(signature)}" length="${fs.statSync(zip).size}" type="application/octet-stream" />
    </item>
  </channel>
</rss>
`;
fs.writeFileSync(path.join(directory, 'appcast.xml'), xml);
console.log(`Signed update feed for ${app} ${version} (build ${build})`);
