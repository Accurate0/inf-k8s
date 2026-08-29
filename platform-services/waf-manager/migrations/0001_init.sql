create table suppressions (
    cidr text primary key,
    until timestamptz not null
);

create table conflicts (
    id bigserial primary key,
    source text not null,
    message text not null
);

create table decisions (
    id bigserial primary key,
    at timestamptz not null,
    workflow text not null,
    cidr text not null,
    detections bigint not null,
    mode text not null,
    outcome text not null
);

create index decisions_at_idx on decisions (at desc, id desc);
