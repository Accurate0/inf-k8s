<script lang="ts">
  import { enhance } from "$app/forms";
  import { valueTypeLabels, operatorOptions } from "$lib/labels";
  import { ValueType, type Rule } from "@accurate0/feature-flag-client/model";
  import * as Card from "$lib/components/ui/card";
  import * as Table from "$lib/components/ui/table";
  import { Button } from "$lib/components/ui/button";
  import { Input } from "$lib/components/ui/input";
  import { Label } from "$lib/components/ui/label";
  import { Checkbox } from "$lib/components/ui/checkbox";
  import { Badge } from "$lib/components/ui/badge";
  import { Separator } from "$lib/components/ui/separator";
  import FieldSelect from "$lib/components/field-select.svelte";

  let { data, form } = $props();
  const flag = $derived(data.flag);

  const variantOptions = $derived(flag.variants.map((v) => ({ value: v.key, label: v.key })));
  const segmentOptions = $derived([
    { value: "", label: "everyone" },
    ...data.segments.map((s) => ({ value: s.key, label: `in segment “${s.key}”` })),
  ]);
  const modeOptions = [
    { value: "single", label: "a single variant" },
    { value: "split", label: "a weighted split" },
  ];
  // FieldSelect works with string values; operator enums are mapped to/from strings.
  const operatorSelectOptions = operatorOptions.map((o) => ({ value: String(o.value), label: o.label }));

  type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

  // Inline constraint values are edited as JSON text (e.g. `"@anurag.sh"` or
  // `["AU", "NZ"]`) and parsed back into an array on save.
  type EditorConstraint = { attribute: string; operator: string; valuesRaw: string };
  // CNF: groups are AND-combined, constraints within a group are OR-combined.
  type EditorGroup = { constraints: EditorConstraint[] };

  type EditorRule = {
    segmentKey: string;
    mode: "single" | "split";
    variantKey: string;
    distributions: { variantKey: string; weight: number }[];
    constraintGroups: EditorGroup[];
  };

  function parseValues(raw: string): JsonValue[] {
    const trimmed = raw.trim();
    if (trimmed === "") return [];
    try {
      const parsed = JSON.parse(trimmed) as JsonValue;
      return Array.isArray(parsed) ? parsed : [parsed];
    } catch {
      return [raw];
    }
  }

  function toEditorRules(rules: Rule[]): EditorRule[] {
    return rules.map((r) => ({
      segmentKey: r.segmentKey,
      mode: r.distributions.length > 0 ? "split" : "single",
      variantKey: r.variantKey || (flag.variants[0]?.key ?? ""),
      distributions: r.distributions.length > 0 ? r.distributions.map((d) => ({ ...d })) : [],
      constraintGroups: r.constraintGroups.map((g) => ({
        constraints: g.constraints.map((c) => ({
          attribute: c.attribute,
          operator: String(c.operator),
          valuesRaw: JSON.stringify(c.values),
        })),
      })),
    }));
  }

  let rules = $state<EditorRule[]>([]);
  let syncedKey = $state("");
  $effect(() => {
    const stamp = `${flag.key}:${JSON.stringify(flag.rules)}`;
    if (stamp !== syncedKey) {
      rules = toEditorRules(flag.rules);
      syncedKey = stamp;
    }
  });

  const serializedRules = $derived(
    JSON.stringify(
      rules.map((r) => ({
        segmentKey: r.segmentKey,
        variantKey: r.mode === "single" ? r.variantKey : "",
        distributions: r.mode === "split" ? r.distributions : [],
        // Drop empty groups (and empty constraints) so they don't persist as noise.
        constraintGroups: r.constraintGroups
          .map((g) => ({
            constraints: g.constraints
              .filter((c) => c.attribute.trim() !== "")
              .map((c) => ({
                attribute: c.attribute,
                operator: Number(c.operator),
                values: parseValues(c.valuesRaw),
              })),
          }))
          .filter((g) => g.constraints.length > 0),
      })),
    ),
  );

  function addRule() {
    rules.push({
      segmentKey: "",
      mode: "single",
      variantKey: flag.variants[0]?.key ?? "",
      distributions: [],
      constraintGroups: [],
    });
  }
  function newConstraint(): EditorConstraint {
    return { attribute: "", operator: String(operatorOptions[0].value), valuesRaw: "" };
  }
  function addGroup(rule: EditorRule) {
    rule.constraintGroups.push({ constraints: [newConstraint()] });
  }
  function addCondition(group: EditorGroup) {
    group.constraints.push(newConstraint());
  }
  function removeConstraint(rule: EditorRule, group: EditorGroup, constraint: EditorConstraint) {
    group.constraints.splice(group.constraints.indexOf(constraint), 1);
    if (group.constraints.length === 0) rule.constraintGroups.splice(rule.constraintGroups.indexOf(group), 1);
  }
  function move(i: number, delta: number) {
    const j = i + delta;
    if (j < 0 || j >= rules.length) return;
    [rules[i], rules[j]] = [rules[j], rules[i]];
  }
  function addDistribution(rule: EditorRule) {
    rule.distributions.push({ variantKey: flag.variants[0]?.key ?? "", weight: 0 });
  }

  // Settings form state.
  let enabled = $state(false);
  let defaultVariantKey = $state("");
  $effect(() => {
    enabled = flag.enabled;
    defaultVariantKey = flag.defaultVariantKey;
  });

  // Typed variant value editor.
  let variantKey = $state("");
  let variantValue = $state("");
  const variantValueJson = $derived.by(() => {
    switch (flag.valueType) {
      case ValueType.VALUE_TYPE_BOOLEAN:
      case ValueType.VALUE_TYPE_INTEGER:
      case ValueType.VALUE_TYPE_FLOAT:
      case ValueType.VALUE_TYPE_OBJECT:
        return variantValue;
      default:
        return JSON.stringify(variantValue);
    }
  });
</script>

<a class="mb-4 inline-block text-sm text-muted-foreground hover:underline" href="/">&larr; Flags</a>

<div class="mb-4 flex items-center gap-2">
  <h2 class="text-xl font-semibold">{flag.key}</h2>
  <span class="text-muted-foreground">({valueTypeLabels[flag.valueType] ?? "?"})</span>
  {#if flag.archived}<Badge variant="secondary">archived</Badge>{/if}
</div>

{#if form?.message}
  <p class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
    {form.message}
  </p>
{/if}

<div class="space-y-6">
  <Card.Root>
    <Card.Header><Card.Title>Settings</Card.Title></Card.Header>
    <Card.Content>
      <form method="POST" action="?/update" use:enhance class="space-y-4">
        <label class="flex items-center gap-2 text-sm">
          <Checkbox bind:checked={enabled} /> Enabled
          <input type="hidden" name="enabled" value={enabled ? "on" : "off"} />
        </label>
        <div class="grid gap-2">
          <Label>Default variant</Label>
          <FieldSelect bind:value={defaultVariantKey} options={variantOptions} name="defaultVariantKey" class="w-48" />
        </div>
        <Button type="submit">Save</Button>
      </form>
    </Card.Content>
  </Card.Root>

  <Card.Root>
    <Card.Header><Card.Title>Variants</Card.Title></Card.Header>
    <Card.Content class="space-y-4">
      <Table.Root>
        <Table.Header>
          <Table.Row>
            <Table.Head>Key</Table.Head>
            <Table.Head>Value</Table.Head>
            <Table.Head class="text-right"></Table.Head>
          </Table.Row>
        </Table.Header>
        <Table.Body>
          {#each flag.variants as v (v.key)}
            <Table.Row>
              <Table.Cell><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{v.key}</code></Table.Cell>
              <Table.Cell><code class="rounded bg-muted px-1.5 py-0.5 text-xs">{JSON.stringify(v.value)}</code></Table.Cell>
              <Table.Cell class="text-right">
                <form method="POST" action="?/deleteVariant" use:enhance>
                  <input type="hidden" name="variantKey" value={v.key} />
                  <Button type="submit" variant="ghost" size="sm" class="text-destructive">delete</Button>
                </form>
              </Table.Cell>
            </Table.Row>
          {/each}
        </Table.Body>
      </Table.Root>

      <form method="POST" action="?/upsertVariant" use:enhance class="flex gap-2">
        <input type="hidden" name="variantKey" value={variantKey} />
        <input type="hidden" name="value" value={variantValueJson} />
        <Input placeholder="variant key" bind:value={variantKey} required class="flex-1" />
        {#if flag.valueType === ValueType.VALUE_TYPE_BOOLEAN}
          <FieldSelect
            bind:value={variantValue}
            options={[
              { value: "true", label: "true" },
              { value: "false", label: "false" },
            ]}
            class="w-32"
          />
        {:else if flag.valueType === ValueType.VALUE_TYPE_INTEGER || flag.valueType === ValueType.VALUE_TYPE_FLOAT}
          <Input type="number" placeholder="number" bind:value={variantValue} required class="flex-1" />
        {:else if flag.valueType === ValueType.VALUE_TYPE_OBJECT}
          <Input placeholder="JSON object" bind:value={variantValue} required class="flex-1" />
        {:else}
          <Input placeholder="string value" bind:value={variantValue} required class="flex-1" />
        {/if}
        <Button type="submit" variant="outline">Add / update</Button>
      </form>
    </Card.Content>
  </Card.Root>

  <Card.Root>
    <Card.Header>
      <Card.Title>Targeting rules</Card.Title>
      <Card.Description>Evaluated top to bottom; the first matching rule wins. Falls through to the default variant. Use a “flag matches” condition (attribute = flag key, value = variant) to depend on another flag.</Card.Description>
    </Card.Header>
    <Card.Content>
      <form method="POST" action="?/setRules" use:enhance class="space-y-3">
        <input type="hidden" name="rules" value={serializedRules} />
        {#each rules as rule, i (i)}
          <div class="rounded-md border bg-muted/30 p-3">
            <div class="flex flex-wrap items-center gap-2">
              <span class="font-bold text-muted-foreground">#{i + 1}</span>
              <span class="text-sm">When</span>
              <FieldSelect bind:value={rule.segmentKey} options={segmentOptions} class="w-56" />
              <span class="text-sm">serve</span>
              <FieldSelect bind:value={rule.mode} options={modeOptions} class="w-44" />
              <span class="flex-1"></span>
              <Button type="button" variant="outline" size="icon" onclick={() => move(i, -1)} disabled={i === 0}>↑</Button>
              <Button type="button" variant="outline" size="icon" onclick={() => move(i, 1)} disabled={i === rules.length - 1}>↓</Button>
              <Button type="button" variant="ghost" size="sm" class="text-destructive" onclick={() => rules.splice(i, 1)}>remove</Button>
            </div>

            <div class="mt-2 ml-6 flex flex-col gap-1.5">
              {#each rule.constraintGroups as group, gi (group)}
                {#if gi > 0}
                  <span class="text-xs font-medium text-muted-foreground">and</span>
                {/if}
                <div class="flex flex-col gap-1.5 rounded-md border border-dashed bg-background/40 p-2">
                  {#each group.constraints as constraint, ci (constraint)}
                    <div class="flex items-center gap-2 text-sm">
                      <span class="w-8 text-right text-xs text-muted-foreground">{ci === 0 ? "if" : "or"}</span>
                      <Input placeholder="attribute" bind:value={constraint.attribute} class="h-8 w-36" />
                      <FieldSelect bind:value={constraint.operator} options={operatorSelectOptions} class="h-8 w-32" />
                      <Input placeholder={`value or ["a","b"]`} bind:value={constraint.valuesRaw} class="h-8 w-48" />
                      <Button type="button" variant="ghost" size="icon" class="size-7 text-muted-foreground hover:text-destructive" onclick={() => removeConstraint(rule, group, constraint)}>×</Button>
                    </div>
                  {/each}
                  <button type="button" class="ml-10 w-fit text-xs text-muted-foreground hover:text-foreground" onclick={() => addCondition(group)}>
                    + or
                  </button>
                </div>
              {/each}
              <button type="button" class="w-fit text-xs text-muted-foreground hover:text-foreground" onclick={() => addGroup(rule)}>
                + add attribute condition
              </button>
            </div>

            {#if rule.mode === "single"}
              <div class="mt-3 ml-6 flex items-center gap-2">
                <span class="text-sm">variant</span>
                <FieldSelect bind:value={rule.variantKey} options={variantOptions} class="w-44" />
              </div>
            {:else}
              <div class="mt-3 ml-6 space-y-2">
                {#each rule.distributions as dist (dist)}
                  <div class="flex items-center gap-2">
                    <FieldSelect bind:value={dist.variantKey} options={variantOptions} class="w-44" />
                    <Input type="number" min="0" max="100" bind:value={dist.weight} class="w-24" />
                    <span class="text-sm text-muted-foreground">%</span>
                    <Button type="button" variant="ghost" size="icon" class="text-destructive" onclick={() => rule.distributions.splice(rule.distributions.indexOf(dist), 1)}>×</Button>
                  </div>
                {/each}
                <div class="flex items-center gap-3">
                  <Button type="button" variant="outline" size="sm" onclick={() => addDistribution(rule)}>add variant</Button>
                  <span class="text-sm text-muted-foreground">total: {rule.distributions.reduce((a, d) => a + Number(d.weight || 0), 0)}%</span>
                </div>
              </div>
            {/if}
          </div>
        {/each}

        <div class="flex gap-3">
          <Button type="button" variant="outline" onclick={addRule}>Add rule</Button>
          <Button type="submit">Save rules</Button>
        </div>
      </form>
    </Card.Content>
  </Card.Root>

  <Card.Root class="border-destructive/40">
    <Card.Header><Card.Title>Danger zone</Card.Title></Card.Header>
    <Card.Content class="flex items-center gap-3">
      <form method="POST" action="?/archive" use:enhance>
        <input type="hidden" name="archived" value={(!flag.archived).toString()} />
        <Button type="submit" variant="outline">{flag.archived ? "Unarchive" : "Archive"}</Button>
      </form>
      <Separator orientation="vertical" class="h-6" />
      <form method="POST" action="?/delete" use:enhance>
        <Button type="submit" variant="destructive">Delete flag</Button>
      </form>
    </Card.Content>
  </Card.Root>
</div>
