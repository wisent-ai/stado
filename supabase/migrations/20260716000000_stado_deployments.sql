begin;

create table if not exists public.stado_infrastructure_targets (
    id uuid primary key default gen_random_uuid(),
    reported_by uuid not null references auth.users(id) on delete cascade,
    provider text not null check (provider in ('local', 'gcp', 'aws', 'azure')),
    kind text not null check (kind in ('device', 'account', 'project', 'subscription')),
    external_id text not null,
    display_name text not null,
    metadata jsonb not null default '{}'::jsonb,
    capabilities text[] not null default '{}',
    last_seen_at timestamptz not null default now(),
    created_at timestamptz not null default now(),
    unique (reported_by, provider, kind, external_id)
);

create table if not exists public.stado_deployments (
    id uuid primary key default gen_random_uuid(),
    created_by uuid not null references auth.users(id) on delete restrict,
    home_org_id uuid references public.organizations(id) on delete set null,
    target_id uuid references public.stado_infrastructure_targets(id) on delete set null,
    name text not null check (char_length(name) between 1 and 120),
    provider text not null check (provider in ('local', 'gcp', 'aws', 'azure')),
    status text not null default 'provisioning'
        check (status in ('provisioning', 'ready', 'degraded', 'failed', 'deleting')),
    endpoint text,
    region text,
    target_summary jsonb not null default '{}'::jsonb,
    last_health_at timestamptz,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists public.stado_deployment_grants (
    id uuid primary key default gen_random_uuid(),
    deployment_id uuid not null references public.stado_deployments(id) on delete cascade,
    subject_kind text not null
        check (subject_kind in ('user', 'organization', 'organization_role')),
    subject_id uuid not null,
    subject_role text,
    permissions text[] not null,
    created_by uuid not null references auth.users(id) on delete restrict,
    created_at timestamptz not null default now(),
    check (
        (subject_kind = 'organization_role' and subject_role in ('owner', 'admin', 'member'))
        or (subject_kind <> 'organization_role' and subject_role is null)
    ),
    check (permissions <@ array['view', 'submit', 'operate', 'admin']::text[]),
    check (cardinality(permissions) > 0),
    unique nulls not distinct (deployment_id, subject_kind, subject_id, subject_role)
);

create index if not exists stado_targets_reported_by_idx
    on public.stado_infrastructure_targets(reported_by, last_seen_at desc);
create index if not exists stado_deployments_creator_idx
    on public.stado_deployments(created_by, updated_at desc);
create index if not exists stado_deployments_home_org_idx
    on public.stado_deployments(home_org_id, updated_at desc);
create index if not exists stado_grants_deployment_idx
    on public.stado_deployment_grants(deployment_id);
create index if not exists stado_grants_subject_idx
    on public.stado_deployment_grants(subject_kind, subject_id);

create or replace function public.stado_can_access(
    target_deployment_id uuid,
    requested_permission text default 'view'
)
returns boolean
language sql
stable
security definer
set search_path = public
as $$
    select exists (
        select 1
        from public.stado_deployments d
        where d.id = target_deployment_id
          and (
              d.created_by = auth.uid()
              or exists (
                  select 1
                  from public.stado_deployment_grants g
                  where g.deployment_id = d.id
                    and (
                        'admin' = any(g.permissions)
                        or requested_permission = any(g.permissions)
                    )
                    and (
                        (g.subject_kind = 'user' and g.subject_id = auth.uid())
                        or (
                            g.subject_kind = 'organization'
                            and exists (
                                select 1
                                from public.organization_members om
                                where om.org_id = g.subject_id
                                  and om.user_id = auth.uid()
                            )
                        )
                        or (
                            g.subject_kind = 'organization_role'
                            and exists (
                                select 1
                                from public.organization_members om
                                where om.org_id = g.subject_id
                                  and om.user_id = auth.uid()
                                  and om.role = g.subject_role
                            )
                        )
                    )
              )
          )
    );
$$;

revoke all on function public.stado_can_access(uuid, text) from public;
grant execute on function public.stado_can_access(uuid, text) to authenticated;

create or replace function public.stado_set_updated_at()
returns trigger
language plpgsql
set search_path = public
as $$
begin
    new.updated_at = now();
    return new;
end;
$$;

create or replace function public.stado_protect_deployment_ownership()
returns trigger
language plpgsql
set search_path = public
as $$
begin
    if auth.role() <> 'service_role'
       and (new.created_by <> old.created_by
            or new.home_org_id is distinct from old.home_org_id) then
        raise exception 'deployment ownership cannot be changed';
    end if;
    return new;
end;
$$;

create trigger stado_deployments_set_updated_at
before update on public.stado_deployments
for each row execute function public.stado_set_updated_at();

create trigger stado_deployments_protect_ownership
before update on public.stado_deployments
for each row execute function public.stado_protect_deployment_ownership();

alter table public.stado_infrastructure_targets enable row level security;
alter table public.stado_deployments enable row level security;
alter table public.stado_deployment_grants enable row level security;

create policy stado_targets_select_own
on public.stado_infrastructure_targets for select
to authenticated
using (reported_by = auth.uid());

create policy stado_targets_insert_own
on public.stado_infrastructure_targets for insert
to authenticated
with check (reported_by = auth.uid());

create policy stado_targets_update_own
on public.stado_infrastructure_targets for update
to authenticated
using (reported_by = auth.uid())
with check (reported_by = auth.uid());

create policy stado_targets_delete_own
on public.stado_infrastructure_targets for delete
to authenticated
using (reported_by = auth.uid());

create policy stado_deployments_select_shared
on public.stado_deployments for select
to authenticated
using (public.stado_can_access(id, 'view'));

create policy stado_deployments_insert_own
on public.stado_deployments for insert
to authenticated
with check (
    created_by = auth.uid()
    and (
        home_org_id is null
        or exists (
            select 1 from public.organization_members om
            where om.org_id = home_org_id and om.user_id = auth.uid()
        )
    )
    and (
        target_id is null
        or exists (
            select 1 from public.stado_infrastructure_targets t
            where t.id = target_id and t.reported_by = auth.uid()
        )
    )
);

create policy stado_deployments_update_admin
on public.stado_deployments for update
to authenticated
using (public.stado_can_access(id, 'admin'))
with check (public.stado_can_access(id, 'admin'));

create policy stado_deployments_delete_admin
on public.stado_deployments for delete
to authenticated
using (public.stado_can_access(id, 'admin'));

create policy stado_grants_select_admin
on public.stado_deployment_grants for select
to authenticated
using (public.stado_can_access(deployment_id, 'admin'));

create policy stado_grants_insert_admin
on public.stado_deployment_grants for insert
to authenticated
with check (
    created_by = auth.uid()
    and public.stado_can_access(deployment_id, 'admin')
);

create policy stado_grants_update_admin
on public.stado_deployment_grants for update
to authenticated
using (public.stado_can_access(deployment_id, 'admin'))
with check (public.stado_can_access(deployment_id, 'admin'));

create policy stado_grants_delete_admin
on public.stado_deployment_grants for delete
to authenticated
using (public.stado_can_access(deployment_id, 'admin'));

grant select, insert, update, delete on public.stado_infrastructure_targets to authenticated;
grant select, insert, update, delete on public.stado_deployments to authenticated;
grant select, insert, update, delete on public.stado_deployment_grants to authenticated;

commit;
