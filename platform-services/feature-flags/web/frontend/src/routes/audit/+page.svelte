<script lang="ts">
  import type { FlagChange } from "@accurate0/feature-flag-client/model";
  import * as Table from "$lib/components/ui/table";
  import { Badge } from "$lib/components/ui/badge";

  let { data } = $props();

  function formatTime(iso: string): string {
    const date = new Date(iso);
    return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
  }

  // The detail payload is a JSON object string; pretty-print it, falling back to the
  // raw string if the backend ever sends something unparsable.
  function formatDetail(change: FlagChange): string {
    if (!change.detail || change.detail === "null" || change.detail === "{}") return "";
    try {
      return JSON.stringify(JSON.parse(change.detail));
    } catch {
      return change.detail;
    }
  }
</script>

<h2 class="mb-4 text-xl font-semibold">Audit log</h2>
<p class="mb-4 text-sm text-muted-foreground">
  Every admin mutation, newest first, with the actor that made it and the config version it produced.
</p>

<div class="rounded-lg border bg-card">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>When</Table.Head>
        <Table.Head>Actor</Table.Head>
        <Table.Head>Action</Table.Head>
        <Table.Head>Target</Table.Head>
        <Table.Head>Detail</Table.Head>
        <Table.Head class="text-right">Version</Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each data.changes as change (change.id)}
        <Table.Row>
          <Table.Cell class="whitespace-nowrap text-muted-foreground">{formatTime(change.createdAt)}</Table.Cell>
          <Table.Cell>{change.actor}</Table.Cell>
          <Table.Cell><Badge variant="secondary">{change.action}</Badge></Table.Cell>
          <Table.Cell>
            <span class="text-muted-foreground">{change.targetKind}</span>
            <code class="rounded bg-muted px-1.5 py-0.5 text-xs">{change.targetKey}</code>
          </Table.Cell>
          <Table.Cell class="font-mono text-xs text-muted-foreground">{formatDetail(change)}</Table.Cell>
          <Table.Cell class="text-right tabular-nums">{change.version}</Table.Cell>
        </Table.Row>
      {/each}
      {#if data.changes.length === 0}
        <Table.Row>
          <Table.Cell colspan={6} class="text-center text-muted-foreground">No changes recorded yet.</Table.Cell>
        </Table.Row>
      {/if}
    </Table.Body>
  </Table.Root>
</div>
