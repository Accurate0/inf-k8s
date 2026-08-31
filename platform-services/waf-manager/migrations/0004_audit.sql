create table audit (
    id bigserial primary key,
    at timestamptz not null,
    actor text not null,
    action text not null,
    target text not null,
    detail text
);

create index audit_at_idx on audit (at desc, id desc);

create index audit_target_idx on audit (target);
