CREATE TABLE holiday_dates (
    date TEXT PRIMARY KEY,
    year BIGINT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('holiday', 'adjusted_workday')),
    source_url TEXT NOT NULL,
    imported_at BIGINT NOT NULL
);
