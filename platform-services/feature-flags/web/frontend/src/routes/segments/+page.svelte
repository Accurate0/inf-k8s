<script lang="ts">
  import { enhance } from "$app/forms";
  import { operatorLabels, operatorOptions } from "$lib/labels";
  import type { Segment } from "@accurate0/feature-flag-client/model";
  import * as Card from "$lib/components/ui/card";
  import * as Table from "$lib/components/ui/table";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import FieldSelect from "$lib/components/field-select.svelte";

  let { data, form } = $props();

  const operatorSelectOptions = operatorOptions.map((o) => ({ value: o.label, label: o.label }));

  type ConstraintRow = { attribute: string; operator: string; values: string };
  let key = $state("");
  let name = $state("");
  let constraints = $state<ConstraintRow[]>([{ attribute: "", operator: "eq", values: '""' }]);

  function constraintsText(s: Segment): string {
    return s.constraints
      .map((c) => `${c.attribute} ${operatorLabels[c.operator] ?? "?"} ${JSON.stringify(c.values)}`)
      .join("; ");
  }

  function edit(s: Segment) {
    key = s.key;
    name = s.name;
    constraints = s.constraints.map((c) => ({
      attribute: c.attribute,
      operator: operatorLabels[c.operator] ?? "eq",
      values: JSON.stringify(c.values),
    }));
  }

  function parseValues(raw: string): unknown {
    try {
      return JSON.parse(raw);
    } catch {
      return raw;
    }
  }

  const constraintsJson = $derived(
    JSON.stringify(
      constraints
        .filter((c) => c.attribute)
        .map((c) => ({ attribute: c.attribute, operator: c.operator, values: parseValues(c.values) })),
    ),
  );
</script>

<h2 class="mb-4 text-xl font-semibold">Segments</h2>

{#if form?.message}
  <p class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
    {form.message}
  </p>
{/if}

<div class="space-y-6">
  <div class="rounded-lg border bg-card">
    <Table.Root>
      <Table.Header>
        <Table.Row>
          <Table.Head>Key</Table.Head>
          <Table.Head>Name</Table.Head>
          <Table.Head>Constraints</Table.Head>
          <Table.Head class="text-right"></Table.Head>
        </Table.Row>
      </Table.Header>
      <Table.Body>
        {#each data.segments as segment (segment.key)}
          <Table.Row>
            <Table.Cell><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{segment.key}</code></Table.Cell>
            <Table.Cell>{segment.name}</Table.Cell>
            <Table.Cell class="text-muted-foreground">{constraintsText(segment)}</Table.Cell>
            <Table.Cell class="text-right">
              <Button variant="ghost" size="sm" onclick={() => edit(segment)}>edit</Button>
              <form method="POST" action="?/delete" use:enhance class="inline">
                <input type="hidden" name="key" value={segment.key} />
                <Button type="submit" variant="ghost" size="sm" class="text-destructive">delete</Button>
              </form>
            </Table.Cell>
          </Table.Row>
        {/each}
        {#if data.segments.length === 0}
          <Table.Row>
            <Table.Cell colspan={4} class="text-center text-muted-foreground">No segments yet.</Table.Cell>
          </Table.Row>
        {/if}
      </Table.Body>
    </Table.Root>
  </div>

  <Card.Root>
    <Card.Header>
      <Card.Title>Create / update segment</Card.Title>
      <Card.Description>A context matches the segment only when every constraint matches.</Card.Description>
    </Card.Header>
    <Card.Content>
      <form method="POST" action="?/upsert" use:enhance class="space-y-4">
        <input type="hidden" name="constraints" value={constraintsJson} />
        <div class="grid grid-cols-2 gap-4">
          <div class="grid gap-2">
            <Label for="seg-key">Key</Label>
            <Input id="seg-key" name="key" required bind:value={key} />
          </div>
          <div class="grid gap-2">
            <Label for="seg-name">Name</Label>
            <Input id="seg-name" name="name" required bind:value={name} />
          </div>
        </div>

        <div class="space-y-2">
          <Label>Constraints</Label>
          {#each constraints as constraint, i (i)}
            <div class="flex gap-2">
              <Input placeholder="attribute" bind:value={constraint.attribute} class="flex-1" />
              <FieldSelect bind:value={constraint.operator} options={operatorSelectOptions} class="w-36" />
              <Input placeholder="value(s) JSON" bind:value={constraint.values} class="flex-1" />
              <Button type="button" variant="ghost" size="icon" onclick={() => constraints.splice(i, 1)}>×</Button>
            </div>
          {/each}
          <Button type="button" variant="outline" size="sm" onclick={() => constraints.push({ attribute: "", operator: "eq", values: '""' })}>
            Add constraint
          </Button>
        </div>

        <Button type="submit">Save</Button>
      </form>
    </Card.Content>
  </Card.Root>
</div>
