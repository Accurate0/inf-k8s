<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children,
		heading,
		...restProps
	}: CommandPrimitive.GroupProps = $props();
</script>

<CommandPrimitive.Group
	bind:ref
	class={cn(
		"text-foreground [&_[data-slot=command-group-heading]]:text-muted-foreground overflow-hidden p-1 [&_[data-slot=command-group-heading]]:px-2 [&_[data-slot=command-group-heading]]:py-1.5 [&_[data-slot=command-group-heading]]:text-xs [&_[data-slot=command-group-heading]]:font-medium",
		className
	)}
	{...restProps}
>
	{#if heading}
		<div data-slot="command-group-heading">{heading}</div>
	{/if}
	{@render children?.()}
</CommandPrimitive.Group>
