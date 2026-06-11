<script lang="ts">
  import { enhance } from "$app/forms";
  import { valueTypeLabels } from "$lib/labels";
  import * as Table from "$lib/components/ui/table";
  import { Button, buttonVariants } from "$lib/components/ui/button";
  import { Badge } from "$lib/components/ui/badge";

  let { data, form } = $props();
</script>

<div class="mb-4 flex items-center justify-between">
  <h2 class="text-xl font-semibold">Flags</h2>
  <div class="flex items-center gap-3">
    <a class={buttonVariants({ variant: "ghost", size: "sm" })} href={data.includeArchived ? "/" : "/?archived=1"}>
      {data.includeArchived ? "Hide archived" : "Show archived"}
    </a>
    <a class={buttonVariants({ size: "sm" })} href="/flags/new">New flag</a>
  </div>
</div>

{#if form?.message}
  <p class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
    {form.message}
  </p>
{/if}

<div class="rounded-lg border bg-card">
  <Table.Root>
    <Table.Header>
      <Table.Row>
        <Table.Head>Key</Table.Head>
        <Table.Head>Type</Table.Head>
        <Table.Head>Default</Table.Head>
        <Table.Head>Enabled</Table.Head>
        <Table.Head class="text-right">Edit</Table.Head>
      </Table.Row>
    </Table.Header>
    <Table.Body>
      {#each data.flags as flag (flag.key)}
        <Table.Row class={flag.archived ? "opacity-55" : ""}>
          <Table.Cell>
            <a class="font-medium hover:underline" href="/flags/{encodeURIComponent(flag.key)}">{flag.key}</a>
            {#if flag.archived}<Badge variant="secondary" class="ml-2">archived</Badge>{/if}
          </Table.Cell>
          <Table.Cell>{valueTypeLabels[flag.valueType] ?? "?"}</Table.Cell>
          <Table.Cell><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{flag.defaultVariantKey}</code></Table.Cell>
          <Table.Cell>
            <form method="POST" action="?/toggle" use:enhance>
              <input type="hidden" name="key" value={flag.key} />
              <input type="hidden" name="defaultVariantKey" value={flag.defaultVariantKey} />
              <input type="hidden" name="enabled" value={(!flag.enabled).toString()} />
              <Button type="submit" variant={flag.enabled ? "default" : "outline"} size="sm">
                {flag.enabled ? "on" : "off"}
              </Button>
            </form>
          </Table.Cell>
          <Table.Cell class="text-right">
            <a class={buttonVariants({ variant: "ghost", size: "sm" })} href="/flags/{encodeURIComponent(flag.key)}">edit</a>
          </Table.Cell>
        </Table.Row>
      {/each}
      {#if data.flags.length === 0}
        <Table.Row>
          <Table.Cell colspan={5} class="text-center text-muted-foreground">No flags yet.</Table.Cell>
        </Table.Row>
      {/if}
    </Table.Body>
  </Table.Root>
</div>
