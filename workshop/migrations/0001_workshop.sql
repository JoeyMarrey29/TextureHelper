CREATE TABLE logins (
  id TEXT PRIMARY KEY,
  challenge TEXT NOT NULL,
  loopback_port INTEGER NOT NULL,
  callback_path TEXT NOT NULL,
  handoff_hash TEXT,
  browser_hash TEXT,
  steam_id TEXT,
  expires INTEGER NOT NULL,
  consumed INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE nonces (nonce TEXT PRIMARY KEY, expires INTEGER NOT NULL);
CREATE TABLE sessions (hash TEXT PRIMARY KEY, steam_id TEXT NOT NULL, expires INTEGER NOT NULL);
CREATE TABLE bans (steam_id TEXT PRIMARY KEY, reason TEXT NOT NULL, created INTEGER NOT NULL);
CREATE TABLE quotas (key TEXT PRIMARY KEY, count INTEGER NOT NULL, expires INTEGER);
CREATE INDEX quota_expiry ON quotas(expires);
CREATE TABLE skins (
  id TEXT PRIMARY KEY,
  steam_id TEXT NOT NULL,
  title TEXT NOT NULL,
  description TEXT NOT NULL,
  model TEXT NOT NULL,
  license TEXT NOT NULL,
  parent TEXT,
  hash TEXT NOT NULL,
  preview_hash TEXT NOT NULL,
  release_id INTEGER,
  status TEXT NOT NULL,
  created INTEGER NOT NULL
);
CREATE INDEX skins_public ON skins(status, created DESC);
CREATE TABLE audit (
  id INTEGER PRIMARY KEY,
  actor TEXT NOT NULL,
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  created INTEGER NOT NULL
);
