ALTER TABLE device_apps ADD COLUMN added_at TEXT;

CREATE INDEX device_apps_added_at ON device_apps (added_at);

CREATE TABLE store_pins (
  slot TEXT PRIMARY KEY CHECK (slot IN ('featured')),
  app_id TEXT NOT NULL,
  source_url TEXT NOT NULL,
  pinned_at TEXT NOT NULL
);
