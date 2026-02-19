<script lang="ts">
	import * as Command from "$lib/components/ui/command/index.js";
	import * as Popover from "$lib/components/ui/popover/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import { Check, ChevronsUpDown, Plus } from "@lucide/svelte";
	import { cn } from "$lib/utils.js";

	let {
		options = [],
		placeholder = "Select...",
		emptyText = "No results found.",
		onSelect,
		class: className
	} = $props<{
		options: string[];
		placeholder?: string;
		emptyText?: string;
		onSelect: (value: string) => void;
		class?: string;
	}>();

	let open = $state(false);
	let inputValue = $state("");

	function closeAndSelect(val: string) {
		open = false;
		onSelect(val);
		inputValue = "";
	}
</script>

<Popover.Root bind:open>
	<Popover.Trigger
		class={cn(
			"border-input data-[placeholder]:text-muted-foreground [&_svg:not([class*='text-'])]:text-muted-foreground focus-visible:border-ring focus-visible:ring-ring/50 aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive dark:bg-input/30 dark:hover:bg-input/50 flex w-full items-center justify-between gap-2 rounded-md border bg-transparent px-3 py-2 text-sm whitespace-nowrap shadow-xs transition-[color,box-shadow] outline-none select-none focus-visible:ring-[3px] disabled:cursor-not-allowed disabled:opacity-50 h-9 font-normal",
			className
		)}
	>
		{placeholder}
		<ChevronsUpDown class="ml-2 h-4 w-4 shrink-0 opacity-50" />
	</Popover.Trigger>
	<Popover.Content class="w-[var(--bits-popover-anchor-width)] p-0" align="start">
		<Command.Root>
			<Command.Input placeholder={placeholder} bind:value={inputValue} />
			<Command.List>
				<Command.Empty>
					<div class="flex flex-col items-center gap-2 py-2">
						<span class="text-xs text-muted-foreground">{emptyText}</span>
						{#if inputValue}
							<Button 
								variant="ghost" 
								size="sm" 
								class="h-8 px-2 text-xs"
								onclick={() => closeAndSelect(inputValue)}
							>
								<Plus class="mr-2 h-3 w-3" />
								Add "{inputValue}"
							</Button>
						{/if}
					</div>
				</Command.Empty>
				<Command.Group>
					{#each options as option}
						<Command.Item
							value={option}
							onSelect={() => closeAndSelect(option)}
						>
							<Check
								class={cn(
									"mr-2 h-4 w-4 opacity-0"
								)}
							/>
							{option}
						</Command.Item>
					{/each}
				</Command.Group>
			</Command.List>
		</Command.Root>
	</Popover.Content>
</Popover.Root>
