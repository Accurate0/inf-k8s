<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import { Button } from '$lib/components/ui/button';
	import { Loader2, Bell, Pencil, Trash2, Plus } from '@lucide/svelte';
	import type { EventResponse } from '$lib/api';

	let {
		namespace,
		events = [],
		loading = false,
		onAddEvent,
		onEditEvent,
		onDeleteEvent
	} = $props();
</script>

<div class="space-y-4">
	<div class="flex items-center justify-between">
		<h2 class="text-2xl font-semibold tracking-tight">
			Events in <span class="text-primary">{namespace}</span>
		</h2>
	</div>

	<Card.Root>
		<Card.Content class="p-0">
			<Table.Root>
				<Table.Header>
					<Table.Row>
						<Table.Head class="pl-6">ID</Table.Head>
						<Table.Head>Keys</Table.Head>
						<Table.Head>Notification</Table.Head>
						<Table.Head>Created At</Table.Head>
						<Table.Head class="pr-6 text-right">Actions</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#if loading && events.length === 0}
						<Table.Row>
							<Table.Cell colspan={5} class="h-32 text-center">
								<div class="flex flex-col items-center justify-center space-y-2">
									<Loader2 class="h-8 w-8 animate-spin text-muted-foreground" />
									<p class="animate-pulse text-muted-foreground">Loading events...</p>
								</div>
							</Table.Cell>
						</Table.Row>
					{:else if events.length === 0}
						<Table.Row>
							<Table.Cell colspan={5} class="h-32 text-center">
								<div class="flex flex-col items-center justify-center space-y-2">
									<Bell class="h-12 w-12 text-muted-foreground/50" />
									<p class="text-muted-foreground">No events found in this namespace.</p>
								</div>
							</Table.Cell>
						</Table.Row>
					{:else}
						{#each events as event (event.id)}
							<Table.Row>
								<Table.Cell class="pl-6 font-mono text-xs font-medium">{event.id}</Table.Cell>
								<Table.Cell>
									<div class="flex flex-wrap gap-1">
										{#each event.keys as key}
											<span
												class="inline-flex items-center rounded-md bg-muted px-2 py-1 text-xs font-medium ring-1 ring-gray-500/10 ring-inset"
											>
												{key}
											</span>
										{/each}
									</div>
								</Table.Cell>
								<Table.Cell>
									<div class="flex flex-col gap-1 text-xs">
										<div class="flex items-center gap-2">
											<span
												class="rounded bg-primary/10 px-1 text-[10px] font-semibold text-primary uppercase"
											>
												{event.notify.type}
											</span>
											<span class="text-muted-foreground italic">
												{event.notify.method}
											</span>
										</div>
										<div
											class="max-w-[300px] truncate text-muted-foreground"
											title={event.notify.urls.join(', ')}
										>
											{event.notify.urls.join(', ')}
										</div>
									</div>
								</Table.Cell>
								<Table.Cell class="text-sm text-muted-foreground">
									{new Date(event.created_at).toLocaleString()}
								</Table.Cell>
								<Table.Cell class="pr-6 text-right">
									<div class="flex justify-end gap-1">
										<Button
											variant="ghost"
											size="sm"
											onclick={() => onEditEvent(event)}
											disabled={loading}
											class="h-8 w-8 p-0"
										>
											<Pencil class="h-4 w-4" />
										</Button>
										<Button
											variant="ghost"
											size="sm"
											onclick={() => onDeleteEvent(event.id)}
											disabled={loading}
											class="h-8 w-8 p-0 text-destructive hover:bg-destructive/10"
										>
											<Trash2 class="h-4 w-4" />
										</Button>
									</div>
								</Table.Cell>
							</Table.Row>
						{/each}
					{/if}

					<!-- Add Event Row -->
					<Table.Row
						class="group cursor-pointer transition-colors hover:bg-muted/50"
						onclick={onAddEvent}
					>
						<Table.Cell class="px-6 py-4" colspan={5}>
							<div
								class="flex items-center gap-3 text-muted-foreground transition-colors group-hover:text-foreground"
							>
								<div
									class="flex h-8 w-8 items-center justify-center rounded-lg border-2 border-dashed border-muted-foreground/25 transition-all group-hover:border-primary/50 group-hover:bg-primary/5"
								>
									<Plus class="h-4 w-4" />
								</div>
								<span class="text-sm font-medium"
									>Click to configure a new event notification for this namespace...</span
								>
							</div>
						</Table.Cell>
					</Table.Row>
				</Table.Body>
			</Table.Root>
		</Card.Content>
	</Card.Root>
</div>
