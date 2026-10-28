-- Party Dungeon — schema v1
-- Postgres 16+. All timestamps are timestamptz, stored UTC.
--
-- House rules:
--   1. Nothing is deleted. Rows retire (retired_at / superseded_by) and a trigger
--      refuses DELETE on every table.
--   2. Every foreign key is ON DELETE RESTRICT. A cascade is a delete with extra steps.
--   3. Published content is immutable. An edit produces a new blueprint_version.
--   4. jsonb only where the shape is still moving. Anything joined or filtered on is a column.
--   5. Units are metres and degrees. Yaw is the only rotation until something needs pitch.
--
-- On Postgres 18+, swap gen_random_uuid() for uuidv7() in the defaults for index locality.

create extension if not exists pgcrypto;   -- gen_random_uuid(), digest()

-- ---------------------------------------------------------------------------
-- Enums: the vocabulary the rest of the schema is allowed to use.
-- ---------------------------------------------------------------------------

create type blueprint_kind  as enum ('authored', 'procedural', 'hybrid');
create type instance_status as enum ('live', 'closing', 'closed');
create type gate_state      as enum ('open', 'closed', 'locked', 'broken');
create type run_outcome     as enum ('extracted', 'wiped', 'abandoned', 'timeout');
create type block_kind      as enum ('surface', 'barrier', 'threshold', 'fixture',
                                     'hazard', 'resource', 'spawn', 'item', 'marker');
create type occupancy       as enum ('cell', 'edge', 'face', 'point');
create type traversal       as enum ('walk', 'climb', 'fall', 'swim',
                                     'teleport', 'conditional', 'blocked');
create type asset_kind      as enum ('mesh', 'texture', 'audio');
create type recipe_op       as enum ('union', 'subtract', 'intersect', 'transform', 'material');
create type spawn_kind      as enum ('mob', 'boss', 'npc', 'captive', 'patrol');
create type item_location   as enum ('inventory', 'stash', 'instance', 'vault', 'listing');
create type listing_state   as enum ('open', 'sold', 'cancelled');
create type dungeon_access  as enum ('private', 'friends', 'public');
create type moderation_state as enum ('pending', 'approved', 'rejected');

-- ---------------------------------------------------------------------------
-- Identity
-- ---------------------------------------------------------------------------

create table accounts (
  id          uuid primary key default gen_random_uuid(),
  handle      text not null unique,
  created_at  timestamptz not null default now(),
  closed_at   timestamptz,
  retired_at  timestamptz,
  superseded_by uuid references accounts(id) on delete restrict
);

create table characters (
  id          uuid primary key default gen_random_uuid(),
  account_id  uuid not null references accounts(id) on delete restrict,
  name        text not null,
  class       text not null,
  level       int  not null default 1 check (level between 1 and 60),
  stats       jsonb not null default '{}'::jsonb,
  created_at  timestamptz not null default now(),
  retired_at  timestamptz,
  unique (account_id, name)
);

-- A party is a roster of characters under one leader. Whether a party is one
-- player's four characters or four players is a join-table question, and this
-- shape answers both: the leader owns it, the members join it.
create table parties (
  id               uuid primary key default gen_random_uuid(),
  leader_account_id uuid not null references accounts(id) on delete restrict,
  created_at       timestamptz not null default now(),
  retired_at       timestamptz
);

create table party_members (
  party_id     uuid not null references parties(id) on delete restrict,
  character_id uuid not null references characters(id) on delete restrict,
  joined_at    timestamptz not null default now(),
  left_at      timestamptz,
  primary key (party_id, character_id, joined_at),
  check (left_at is null or left_at >= joined_at)
);

-- ---------------------------------------------------------------------------
-- Assets: the payload lives in object storage, the truth lives here.
-- ---------------------------------------------------------------------------

create table asset_objects (
  id            uuid primary key default gen_random_uuid(),
  sha256        bytea not null unique check (octet_length(sha256) = 32),
  object_key    text  not null unique,
  byte_size     bigint not null check (byte_size > 0),
  mime          text  not null,
  storage_class text  not null default 'standard',
  created_at    timestamptz not null default now(),
  retired_at    timestamptz
);

create table assets (
  id            uuid primary key default gen_random_uuid(),
  kind          asset_kind not null,
  object_id     uuid references asset_objects(id) on delete restrict,  -- null until baked
  bounds_min    real[] not null check (array_length(bounds_min, 1) = 3),
  bounds_max    real[] not null check (array_length(bounds_max, 1) = 3),
  vertex_count  int check (vertex_count >= 0),
  tri_count     int check (tri_count >= 0),
  material_slots jsonb not null default '[]'::jsonb,
  moderation    moderation_state not null default 'approved',
  created_at    timestamptz not null default now(),
  retired_at    timestamptz,
  check (bounds_min[1] <= bounds_max[1]
     and bounds_min[2] <= bounds_max[2]
     and bounds_min[3] <= bounds_max[3])
);

-- One row per convex hull. Points are int16 triples quantised to the bounds,
-- so a full proxy is a couple of kilobytes rather than a second mesh.
create table collision_hulls (
  asset_id    uuid not null references assets(id) on delete restrict,
  idx         smallint not null,
  points      bytea not null check (octet_length(points) % 6 = 0),
  point_count smallint not null check (point_count between 3 and 64),
  primary key (asset_id, idx)
);

create table asset_recipes (
  id              uuid primary key default gen_random_uuid(),
  op              recipe_op not null,
  params          jsonb not null default '{}'::jsonb,
  output_asset_id uuid not null references assets(id) on delete restrict,
  bake_version    int not null default 1,
  baked_at        timestamptz,
  created_at      timestamptz not null default now()
);

create index asset_recipes_output_idx on asset_recipes (output_asset_id);

create table recipe_inputs (
  recipe_id uuid not null references asset_recipes(id) on delete restrict,
  asset_id  uuid not null references assets(id) on delete restrict,
  ordinal   smallint not null check (ordinal > 0),
  primary key (recipe_id, ordinal)
);

-- ---------------------------------------------------------------------------
-- Blocks: behaviour attached to a prefab. What the editor actually places.
-- ---------------------------------------------------------------------------

create table prefabs (
  id         uuid primary key default gen_random_uuid(),
  name       text not null unique,
  asset_id   uuid not null references assets(id) on delete restrict,
  scale      real not null default 1 check (scale > 0),
  created_at timestamptz not null default now(),
  retired_at timestamptz
);

create table block_definitions (
  id             uuid primary key default gen_random_uuid(),
  key            text not null unique,           -- 'wall.stone', 'gate.portcullis'
  kind           block_kind not null,
  prefab_id      uuid references prefabs(id) on delete restrict,
  occupancy      occupancy not null,
  traversal      traversal not null,
  footprint      smallint not null default 1 check (footprint > 0),
  blocks_movement boolean not null default false,
  blocks_sight    boolean not null default false,
  clearance      smallint not null default 1 check (clearance > 0),
  placement      jsonb not null default '{}'::jsonb,  -- snap target, rotations, per-cell limit
  created_at     timestamptz not null default now(),
  retired_at     timestamptz
);

-- ---------------------------------------------------------------------------
-- Layout: a blueprint is edited, a version is frozen.
-- Fragments are blueprints with is_fragment = true: same tables, no instance.
-- ---------------------------------------------------------------------------

create table blueprints (
  id               uuid primary key default gen_random_uuid(),
  owner_account_id uuid not null references accounts(id) on delete restrict,
  name             text not null,
  kind             blueprint_kind not null default 'authored',
  is_fragment      boolean not null default false,
  current_version_id uuid,
  created_at       timestamptz not null default now(),
  retired_at       timestamptz,
  unique (owner_account_id, name)
);

create table blueprint_versions (
  id           uuid primary key default gen_random_uuid(),
  blueprint_id uuid not null references blueprints(id) on delete restrict,
  version_no   int  not null check (version_no > 0),
  published_by uuid references accounts(id) on delete restrict,
  notes        text,
  created_at   timestamptz not null default now(),
  unique (blueprint_id, version_no),
  unique (id, blueprint_id)          -- lets the current-version pointer be checked
);

alter table blueprints
  add constraint blueprints_current_version_fk
  foreign key (current_version_id, id)
  references blueprint_versions (id, blueprint_id) on delete restrict;

create table placements (
  id         uuid primary key default gen_random_uuid(),
  version_id uuid not null references blueprint_versions(id) on delete restrict,
  block_id   uuid not null references block_definitions(id) on delete restrict,
  parent_id  uuid references placements(id) on delete restrict,
  x          real not null,
  y          real not null,
  z          real not null default 0,
  yaw        real not null default 0,
  params     jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  check (parent_id is distinct from id),
  unique (id, version_id)            -- lets gate state prove it points into the same version
);

create index placements_version_idx on placements (version_id);
create index placements_parent_idx  on placements (parent_id) where parent_id is not null;

create or replace function placements_parent_same_version() returns trigger
language plpgsql as $$
declare parent_version uuid;
begin
  if new.parent_id is null then
    return new;
  end if;
  select version_id into parent_version from placements where id = new.parent_id;
  if parent_version is null then
    raise exception 'parent placement % does not exist', new.parent_id;
  end if;
  if parent_version <> new.version_id then
    raise exception 'parent placement lives in another version';
  end if;
  return new;
end $$;

create trigger placements_parent_same_version
before insert or update of parent_id, version_id on placements
for each row execute function placements_parent_same_version();

-- Derived, disposable, rebuildable. Stored only because rebuilding on every
-- join would make load time a function of map size.
create table derived_regions (
  version_id uuid not null references blueprint_versions(id) on delete restrict,
  level      smallint not null,
  hex_q      int not null,
  hex_r      int not null,
  region_id  int not null,
  primary key (version_id, level, hex_q, hex_r)
);

create table derived_navmeshes (
  version_id       uuid not null references blueprint_versions(id) on delete restrict,
  level            smallint not null,
  gate_fingerprint bytea not null check (octet_length(gate_fingerprint) = 32),
  object_key       text not null,
  byte_size        bigint not null check (byte_size > 0),
  poly_count       int,
  built_at         timestamptz not null default now(),
  primary key (version_id, level, gate_fingerprint)
);

-- ---------------------------------------------------------------------------
-- Dungeons and instances
-- ---------------------------------------------------------------------------

-- A player's base: permanent, owned, open to visitors by policy.
create table dungeons (
  id               uuid primary key default gen_random_uuid(),
  owner_account_id uuid not null references accounts(id) on delete restrict,
  blueprint_id     uuid not null references blueprints(id) on delete restrict,
  name             text not null,
  access           dungeon_access not null default 'private',
  created_at       timestamptz not null default now(),
  retired_at       timestamptz
);

create table instances (
  id                   uuid primary key default gen_random_uuid(),
  blueprint_version_id uuid not null references blueprint_versions(id) on delete restrict,
  dungeon_id           uuid references dungeons(id) on delete restrict,  -- null: not a base run
  status               instance_status not null default 'live',
  born_at              timestamptz not null default now(),
  expires_at           timestamptz not null,
  closed_at            timestamptz,
  state                jsonb not null default '{}'::jsonb,
  check (expires_at > born_at),
  check ((status = 'closed') = (closed_at is not null)),
  unique (id, blueprint_version_id)
);

create index instances_live_idx on instances (expires_at) where status = 'live';

create table instance_participants (
  instance_id  uuid not null references instances(id) on delete restrict,
  character_id uuid not null references characters(id) on delete restrict,
  joined_at    timestamptz not null default now(),
  left_at      timestamptz,
  primary key (instance_id, character_id, joined_at),
  check (left_at is null or left_at >= joined_at)
);

create index participants_inside_idx
  on instance_participants (instance_id) where left_at is null;

-- Gate state and spent content carry the version id so the two composite FKs
-- can prove the placement belongs to the version this instance pinned.
create table instance_gates (
  instance_id  uuid not null,
  version_id   uuid not null,
  placement_id uuid not null,
  state        gate_state not null,
  changed_at   timestamptz not null default now(),
  primary key (instance_id, placement_id),
  foreign key (instance_id, version_id)
    references instances (id, blueprint_version_id) on delete restrict,
  foreign key (placement_id, version_id)
    references placements (id, version_id) on delete restrict
);

create table instance_spent (
  instance_id     uuid not null,
  version_id      uuid not null,
  placement_id    uuid not null,
  by_character_id uuid references characters(id) on delete restrict,
  spent_at        timestamptz not null default now(),
  primary key (instance_id, placement_id),
  foreign key (instance_id, version_id)
    references instances (id, blueprint_version_id) on delete restrict,
  foreign key (placement_id, version_id)
    references placements (id, version_id) on delete restrict
);

create table dungeon_visits (
  id           uuid primary key default gen_random_uuid(),
  dungeon_id   uuid not null references dungeons(id) on delete restrict,
  instance_id  uuid not null references instances(id) on delete restrict,
  character_id uuid not null references characters(id) on delete restrict,
  entered_at   timestamptz not null default now(),
  left_at      timestamptz,
  unique (instance_id, character_id),
  check (left_at is null or left_at >= entered_at)
);

create index visits_dungeon_idx on dungeon_visits (dungeon_id, entered_at desc);

create table runs (
  id          uuid primary key default gen_random_uuid(),
  party_id    uuid not null references parties(id) on delete restrict,
  instance_id uuid not null references instances(id) on delete restrict,
  started_at  timestamptz not null default now(),
  ended_at    timestamptz,
  outcome     run_outcome,
  check (ended_at is null or ended_at >= started_at),
  check ((outcome is null) = (ended_at is null))
);

create index runs_instance_idx on runs (instance_id);

-- ---------------------------------------------------------------------------
-- Items and the market. Items are immutable; ownership moves.
-- ---------------------------------------------------------------------------

create table item_definitions (
  id         uuid primary key default gen_random_uuid(),
  key        text not null unique,
  name       text not null,
  slot       text not null,
  asset_id   uuid references assets(id) on delete restrict,
  base_stats jsonb not null default '{}'::jsonb,
  retired_at timestamptz
);

create table runes (
  id         uuid primary key default gen_random_uuid(),
  key        text not null unique,
  name       text not null,
  effect     jsonb not null,
  retired_at timestamptz
);

create table items (
  id                 uuid primary key default gen_random_uuid(),
  definition_id      uuid not null references item_definitions(id) on delete restrict,
  level              int not null default 1 check (level between 1 and 60),
  runes              uuid[] not null default '{}',
  stats              jsonb not null default '{}'::jsonb,
  crafted_by         uuid references characters(id) on delete restrict,
  owner_character_id uuid references characters(id) on delete restrict,
  location           item_location not null default 'stash',
  instance_id        uuid references instances(id) on delete restrict,
  created_at         timestamptz not null default now(),
  retired_at         timestamptz,
  check ((location = 'instance') = (instance_id is not null))
);

create index items_owner_idx on items (owner_character_id, location);

create table listings (
  id                 uuid primary key default gen_random_uuid(),
  item_id            uuid not null references items(id) on delete restrict,
  seller_character_id uuid not null references characters(id) on delete restrict,
  price              bigint not null check (price > 0),
  state              listing_state not null default 'open',
  created_at         timestamptz not null default now(),
  closed_at          timestamptz,
  check ((state = 'open') = (closed_at is null))
);

create unique index listings_open_item_idx on listings (item_id) where state = 'open';

-- ---------------------------------------------------------------------------
-- No deletes. Ever.
-- ---------------------------------------------------------------------------

create or replace function forbid_delete() returns trigger
language plpgsql as $$
begin
  raise exception '% is append-only: retire the row instead of deleting it', tg_table_name;
end $$;

do $$
declare t text;
begin
  foreach t in array array[
    'accounts', 'characters', 'parties', 'party_members',
    'asset_objects', 'assets', 'collision_hulls', 'asset_recipes', 'recipe_inputs',
    'prefabs', 'block_definitions',
    'blueprints', 'blueprint_versions', 'placements',
    'derived_regions', 'derived_navmeshes',
    'dungeons', 'instances', 'instance_participants',
    'instance_gates', 'instance_spent', 'dungeon_visits', 'runs',
    'item_definitions', 'runes', 'items', 'listings'
  ] loop
    execute format(
      'create trigger %I before delete on %I for each row execute function forbid_delete()',
      t || '_no_delete', t
    );
  end loop;
end $$;

-- The application role never gets DELETE or TRUNCATE:
--   revoke delete, truncate on all tables in schema public from party_dungeon;
