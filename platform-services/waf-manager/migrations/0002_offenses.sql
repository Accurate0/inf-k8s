create table offenses (
    cidr text primary key,
    strikes integer not null,
    first_seen timestamptz not null,
    last_seen timestamptz not null
);

create index offenses_last_seen_idx on offenses (last_seen);
