<script lang="ts">
  import { enhance } from "$app/forms";
  import { onMount } from "svelte";
  import { valueTypeLabels, reasonLabels, operatorLabels } from "$lib/labels";
  import type { Flag, Segment } from "@accurate0/feature-flag-client/model";
  import * as Card from "$lib/components/ui/card";
  import * as Table from "$lib/components/ui/table";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import { Textarea } from "$lib/components/ui/textarea";
  import { Badge } from "$lib/components/ui/badge";
  import FieldSelect from "$lib/components/field-select.svelte";

  let { data, form } = $props();

  type Snapshot = { version: number; flags: Flag[]; segments: Segment[] };
  let live = $state<Snapshot | null>(null);
  let connected = $state(false);

  const sortedFlags = $derived([...(live?.flags ?? [])].sort((a, b) => a.key.localeCompare(b.key)));
  const sortedSegments = $derived([...(live?.segments ?? [])].sort((a, b) => a.key.localeCompare(b.key)));

  const flagOptions = $derived([
    { value: "", label: "All flags" },
    ...[...data.flags]
      .sort((a, b) => a.key.localeCompare(b.key))
      .map((f) => ({ value: f.key, label: f.key })),
  ]);
  let selectedFlag = $state(form?.values?.flagKey ?? "");

  const auth = $derived(data.auth);

  onMount(() => {
    const source = new EventSource("/debug/stream");
    source.onopen = () => (connected = true);
    source.onmessage = (e) => (live = JSON.parse(e.data));
    source.onerror = () => (connected = false);
    return () => source.close();
  });

  function constraintsText(s: Segment): string {
    return s.constraints
      .map((c) => `${c.attribute} ${operatorLabels[c.operator] ?? "?"} ${JSON.stringify(c.values)}`)
      .join("; ");
  }
</script>

<h2 class="mb-4 text-xl font-semibold">Debug</h2>

<div class="space-y-6">
  <Card.Root>
    <Card.Header>
      <Card.Title>Auth forwarding</Card.Title>
      <Card.Description>What the gateway forwarded to this request — diagnosing the audit-log actor.</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-2 text-sm">
      <div>x-forwarded-user: <code>{auth.forwardedUser ?? "(absent)"}</code></div>
      <div>Authorization present: <code>{auth.hasAuthorization}</code></div>
      <div>
        access token claims:
        {#if auth.accessTokenClaims}
          {#each auth.accessTokenClaims as c}<Badge variant="outline" class="mr-1">{c}</Badge>{/each}
        {:else}
          <code>(no JWT in Authorization)</code>
        {/if}
      </div>
    </Card.Content>
  </Card.Root>
  <Card.Root>
    <Card.Header>
      <Card.Title>Snapshot stream</Card.Title>
      <Card.Description>The live snapshot, pushed in full on every config change via the server stream.</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-5">
      {#if live}
        <div class="flex gap-8">
          <div><div class="text-2xl font-bold">{live.version}</div><div class="text-sm text-muted-foreground">version</div></div>
          <div><div class="text-2xl font-bold">{live.flags.length}</div><div class="text-sm text-muted-foreground">flags</div></div>
          <div><div class="text-2xl font-bold">{live.segments.length}</div><div class="text-sm text-muted-foreground">segments</div></div>
        </div>

        <div>
          <h4 class="mb-2 text-sm font-semibold">Flags</h4>
          <Table.Root>
            <Table.Header>
              <Table.Row>
                <Table.Head>Key</Table.Head><Table.Head>Type</Table.Head><Table.Head>Enabled</Table.Head>
                <Table.Head>Default</Table.Head><Table.Head>Variants</Table.Head><Table.Head>Rules</Table.Head>
              </Table.Row>
            </Table.Header>
            <Table.Body>
              {#each sortedFlags as flag (flag.key)}
                <Table.Row class={flag.archived ? "opacity-55" : ""}>
                  <Table.Cell><a class="hover:underline" href="/flags/{encodeURIComponent(flag.key)}">{flag.key}</a></Table.Cell>
                  <Table.Cell>{valueTypeLabels[flag.valueType] ?? "?"}</Table.Cell>
                  <Table.Cell>{flag.enabled ? "on" : "off"}</Table.Cell>
                  <Table.Cell><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{flag.defaultVariantKey}</code></Table.Cell>
                  <Table.Cell>{flag.variants.map((v) => v.key).join(", ")}</Table.Cell>
                  <Table.Cell>{flag.rules.length}</Table.Cell>
                </Table.Row>
              {/each}
              {#if sortedFlags.length === 0}
                <Table.Row><Table.Cell colspan={6} class="text-center text-muted-foreground">No flags.</Table.Cell></Table.Row>
              {/if}
            </Table.Body>
          </Table.Root>
        </div>

        <div>
          <h4 class="mb-2 text-sm font-semibold">Segments</h4>
          <Table.Root>
            <Table.Header>
              <Table.Row><Table.Head>Key</Table.Head><Table.Head>Name</Table.Head><Table.Head>Constraints</Table.Head></Table.Row>
            </Table.Header>
            <Table.Body>
              {#each sortedSegments as segment (segment.key)}
                <Table.Row>
                  <Table.Cell><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{segment.key}</code></Table.Cell>
                  <Table.Cell>{segment.name}</Table.Cell>
                  <Table.Cell class="text-muted-foreground">{constraintsText(segment)}</Table.Cell>
                </Table.Row>
              {/each}
              {#if sortedSegments.length === 0}
                <Table.Row><Table.Cell colspan={3} class="text-center text-muted-foreground">No segments.</Table.Cell></Table.Row>
              {/if}
            </Table.Body>
          </Table.Root>
        </div>
      {:else}
        <p class="text-muted-foreground">{connected ? "Waiting for snapshot…" : "Connecting…"}</p>
      {/if}
    </Card.Content>
  </Card.Root>

  <Card.Root>
    <Card.Header>
      <Card.Title>Test evaluation</Card.Title>
      <Card.Description>Resolves a single flag (through its typed RPC) or every flag server-side against the context, exactly as a provider would.</Card.Description>
    </Card.Header>
    <Card.Content class="space-y-4">
      {#if form?.message}
        <p class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">{form.message}</p>
      {/if}
      <form method="POST" action="?/evaluate" use:enhance class="space-y-4">
        <div class="grid gap-2">
          <Label for="flag">Flag</Label>
          <FieldSelect name="flagKey" bind:value={selectedFlag} options={flagOptions} class="w-72" />
        </div>
        <div class="grid gap-2">
          <Label for="tk">Targeting key</Label>
          <Input id="tk" name="targetingKey" value={form?.values?.targetingKey ?? "user-123"} class="w-72" />
        </div>
        <div class="grid gap-2">
          <Label for="attrs">Attributes (JSON)</Label>
          <Textarea id="attrs" name="attributes" rows={4} class="font-mono" value={form?.values?.attributesRaw ?? '{\n  "country": "AU"\n}'} />
        </div>
        <Button type="submit">Evaluate</Button>
      </form>

      {#if form?.flags && form.single}
        {@const f = form.flags[0]}
        <div class="space-y-3 rounded-md border p-4">
          <div class="flex items-center gap-2">
            <a class="font-medium hover:underline" href="/flags/{encodeURIComponent(f.flagKey)}">{f.flagKey}</a>
            <Badge variant="outline">{valueTypeLabels[f.valueType] ?? "?"}</Badge>
            {#if f.meta?.reason}<Badge variant="secondary">{reasonLabels[f.meta.reason] ?? ""}</Badge>{/if}
          </div>
          <dl class="grid grid-cols-[8rem_1fr] gap-x-4 gap-y-1.5 text-sm">
            <dt class="text-muted-foreground">Value</dt>
            <dd><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{JSON.stringify(f.value)}</code></dd>
            <dt class="text-muted-foreground">Variant</dt>
            <dd>{f.meta?.variant || "—"}</dd>
            <dt class="text-muted-foreground">Reason</dt>
            <dd>{f.meta ? (reasonLabels[f.meta.reason] ?? "—") : "—"}</dd>
            {#if f.meta?.errorCode}
              <dt class="text-muted-foreground">Error code</dt>
              <dd class="text-destructive">{f.meta.errorCode}</dd>
            {/if}
            {#if f.meta?.errorMessage}
              <dt class="text-muted-foreground">Error message</dt>
              <dd class="text-destructive">{f.meta.errorMessage}</dd>
            {/if}
          </dl>
          <div>
            <div class="mb-1 text-xs font-semibold text-muted-foreground">Context</div>
            <pre class="overflow-x-auto rounded bg-muted px-2 py-1.5 font-mono text-xs">{JSON.stringify(form.context, null, 2)}</pre>
          </div>
        </div>
      {:else if form?.flags}
        <Table.Root>
          <Table.Header>
            <Table.Row>
              <Table.Head>Flag</Table.Head><Table.Head>Type</Table.Head><Table.Head>Value</Table.Head>
              <Table.Head>Variant</Table.Head><Table.Head>Reason</Table.Head><Table.Head>Error</Table.Head>
            </Table.Row>
          </Table.Header>
          <Table.Body>
            {#each form.flags as f (f.flagKey)}
              <Table.Row>
                <Table.Cell><a class="hover:underline" href="/flags/{encodeURIComponent(f.flagKey)}">{f.flagKey}</a></Table.Cell>
                <Table.Cell>{valueTypeLabels[f.valueType] ?? "?"}</Table.Cell>
                <Table.Cell><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{JSON.stringify(f.value)}</code></Table.Cell>
                <Table.Cell>{f.meta?.variant ?? ""}</Table.Cell>
                <Table.Cell>
                  {#if f.meta?.reason}<Badge variant="secondary">{reasonLabels[f.meta.reason] ?? ""}</Badge>{/if}
                </Table.Cell>
                <Table.Cell class="text-destructive">{f.meta?.errorCode ?? ""}</Table.Cell>
              </Table.Row>
            {/each}
          </Table.Body>
        </Table.Root>
      {/if}
    </Card.Content>
  </Card.Root>
</div>
