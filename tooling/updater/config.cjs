// Public update configuration. The private Ed25519 key stays in Keychain / GitHub Secrets.
// The key is the one every Polterware app has shipped with, so the installed Electron
// Dispatch accepts the GPUI app as its update.
module.exports = {
  version: '2.8.1',
  archiveSha256: '5cddb7695674ef7704268f38eccaee80e3accbf19e61c1689efff5b6116d85be',
  publicKey: 'N32wHSyKvAhuMfDMCvpfv5/oQ6mz/OtopIrzFUyZi8k=',
  repository: 'polterware/dispatch',
  app: 'dispatch',
  product: 'Dispatch',
  feedUrl: 'https://raw.githubusercontent.com/polterware/dispatch/main/updates/dispatch.xml',
};
