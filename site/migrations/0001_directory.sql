CREATE TABLE catalogs (
  url TEXT PRIMARY KEY,
  body TEXT,
  etag TEXT,
  last_modified TEXT,
  name TEXT NOT NULL,
  description TEXT,
  homepage TEXT,
  icon TEXT,
  app_count INTEGER NOT NULL,
  downloads_cors_ok INTEGER,
  checked_at TEXT NOT NULL,
  check_ok INTEGER NOT NULL,
  check_error TEXT
);

CREATE TABLE sources (
  url TEXT PRIMARY KEY REFERENCES catalogs (url),
  status TEXT NOT NULL CHECK (status IN ('quarantined', 'listed', 'attested', 'rejected')),
  submitted_at TEXT NOT NULL,
  reviewed_at TEXT,
  reviewed_by TEXT,
  note TEXT
);

CREATE TABLE devices (
  serial TEXT PRIMARY KEY,
  updated_at TEXT NOT NULL
);

CREATE TABLE device_apps (
  serial TEXT NOT NULL REFERENCES devices (serial),
  app_id TEXT NOT NULL,
  source_url TEXT NOT NULL,
  version TEXT NOT NULL,
  PRIMARY KEY (serial, app_id, source_url)
);

CREATE TABLE install_counts (
  app_id TEXT NOT NULL,
  source_url TEXT NOT NULL,
  version TEXT NOT NULL,
  devices INTEGER NOT NULL,
  PRIMARY KEY (app_id, source_url, version)
);

CREATE TRIGGER device_app_added AFTER INSERT ON device_apps
BEGIN
  INSERT INTO install_counts (app_id, source_url, version, devices)
  VALUES (NEW.app_id, NEW.source_url, NEW.version, 1)
  ON CONFLICT (app_id, source_url, version) DO UPDATE SET devices = devices + 1;
END;

CREATE TRIGGER device_app_removed AFTER DELETE ON device_apps
BEGIN
  UPDATE install_counts SET devices = devices - 1
  WHERE app_id = OLD.app_id AND source_url = OLD.source_url AND version = OLD.version;
  DELETE FROM install_counts
  WHERE app_id = OLD.app_id AND source_url = OLD.source_url AND version = OLD.version AND devices <= 0;
END;

CREATE TABLE rate_limits (
  bucket TEXT NOT NULL,
  key TEXT NOT NULL,
  window_start INTEGER NOT NULL,
  used INTEGER NOT NULL,
  PRIMARY KEY (bucket, key)
);
