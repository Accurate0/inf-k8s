create table allowlist (
    cidr text primary key,
    note text,
    created_by text not null,
    at timestamptz not null
);

create index decisions_workflow_idx on decisions (workflow, at desc);
