<script lang="ts">
	import { Command as CommandPrimitive } from "bits-ui";
	import { cn } from "$lib/utils.js";

	let {
		ref = $bindable(null),
		class: className,
		children,
		...restProps
	}: CommandPrimitive.RootProps = $props();
</script>

<CommandPrimitive.Root
	bind:ref
	class={cn(
		"bg-popover text-popover-foreground flex h-full w-full flex-col overflow-hidden rounded-md",
		className
	)}
	{...restProps}
>
	{@render children?.()}
</CommandPrimitive.Root>
