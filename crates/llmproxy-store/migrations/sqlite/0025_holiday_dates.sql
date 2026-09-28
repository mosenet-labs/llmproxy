CREATE TABLE holiday_dates (
    date TEXT PRIMARY KEY,
    year INTEGER NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('holiday', 'adjusted_workday')),
    source_url TEXT NOT NULL,
    imported_at INTEGER NOT NULL
);
