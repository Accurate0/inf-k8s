<script lang="ts">
	import { onMount } from 'svelte';
	import { browser } from '$app/environment';
	import { listObjects, downloadObject, uploadObject, type ObjectMetadata } from '$lib/api';

	let namespace = 'default'; // Default namespace
	let objects: ObjectMetadata[] = [];
	let loading = false;
	let error: string | null = null;
	let fileInput: HTMLInputElement;
	let selectedFile: File | null = null;

	async function fetchObjects() {
		if (!browser) return;
		loading = true;
		error = null;
		try {
			const fetchedObjects = await listObjects(namespace);
			objects = fetchedObjects;
		} catch (err: any) {
			error = err.message || 'Failed to fetch objects';
		} finally {
			loading = false;
		}
	}

	async function handleUpload() {
		if (!selectedFile) {
			alert('Please select a file to upload.');
			return;
		}
		loading = true;
		error = null;
		try {
			// Use the original file name as the object key
			await uploadObject(namespace, selectedFile.name, selectedFile);
			alert(`File "${selectedFile.name}" uploaded successfully.`);
			selectedFile = null; // Clear selected file
			if (fileInput) fileInput.value = ''; // Clear file input visual
			await fetchObjects(); // Refresh the list
		} catch (err: any) {
			error = err.message || 'Failed to upload file';
		} finally {
			loading = false;
		}
	}

	async function handleDownload(key: string) {
		loading = true;
		error = null;
		try {
			const { blob, filename } = await downloadObject(namespace, key);
			const url = window.URL.createObjectURL(blob);
			const a = document.createElement('a');
			a.href = url;
			a.download = filename;
			document.body.appendChild(a);
			a.click();
			a.remove();
			window.URL.revokeObjectURL(url);
		} catch (err: any) {
			error = err.message || 'Failed to download file';
		} finally {
			loading = false;
		}
	}

	function handleFileChange(event: Event) {
		const target = event.target as HTMLInputElement;
		selectedFile = target.files ? target.files[0] : null;
	}

	function formatSize(bytes: number): string {
		if (bytes === 0) return '0 B';
		const k = 1024;
		const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
		const i = Math.floor(Math.log(bytes) / Math.log(k));
		return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
	}

	// Fetch objects when the component mounts or namespace changes
	onMount(() => {
		fetchObjects();
	});

	// Reactive statement to re-fetch when namespace changes
	// We check for browser to avoid SSR fetch issues
	$: if (browser && namespace) {
		fetchObjects();
	}
</script>

<div class="container mx-auto p-4 max-w-7xl">
	<h1 class="text-3xl font-bold mb-6 text-gray-800">Object Registry Console</h1>

	{#if error}
		<div
			role="alert"
			class="alert alert-error mb-6 flex items-start justify-between bg-red-50 border-red-200 text-red-800 p-4 rounded-lg shadow-sm"
		>
			<div class="flex items-start">
				<svg
					xmlns="http://www.w3.org/2000/svg"
					class="stroke-current shrink-0 h-6 w-6 mr-3 mt-0.5"
					fill="none"
					viewBox="0 0 24 24"
					><path
						stroke-linecap="round"
						stroke-linejoin="round"
						stroke-width="2"
						d="M10 14l2-2m0 0l2-2m-2 2l-2-2m2 2l2 2m7-2a9 9 0 11-18 0 9 9 0 0118 0z"
					/></svg
				>
				<div>
					<h3 class="font-bold">Error Occurred</h3>
					<div class="text-sm opacity-90 break-all">{error}</div>
				</div>
			</div>
			<button class="btn btn-ghost btn-xs" on:click={() => (error = null)}>✕</button>
		</div>
	{/if}

	<div class="bg-white shadow-md rounded-lg p-6 mb-8">
		<div class="flex items-center mb-4">
			<label for="namespace-input" class="text-lg font-medium text-gray-700 mr-3"
				>Namespace:</label
			>
			<input
				id="namespace-input"
				type="text"
				bind:value={namespace}
				class="input input-bordered w-full max-w-xs px-4 py-2 border rounded-md focus:outline-none focus:ring-2 focus:ring-blue-500"
				placeholder="Enter namespace"
			/>
		</div>

		<div class="flex items-center space-x-4">
			<input
				type="file"
				bind:this={fileInput}
				on:change={handleFileChange}
				class="file-input file-input-bordered w-full max-w-xs"
			/>
			<button
				on:click={handleUpload}
				class="btn btn-primary px-6 py-2 rounded-md shadow hover:bg-blue-600 transition duration-200"
				disabled={!selectedFile || loading}
			>
				{#if loading && selectedFile?.name}
					Uploading {selectedFile.name}...
				{:else}
					Upload
				{/if}
			</button>
		</div>
	</div>

	<h2 class="text-2xl font-semibold mb-4 text-gray-800">Objects in "{namespace}"</h2>

	{#if loading && objects.length === 0}
		<div class="flex items-center justify-center p-8">
			<span class="loading loading-spinner loading-lg"></span>
			<p class="ml-3 text-gray-600">Loading objects...</p>
		</div>
	{:else if objects.length === 0 && !error}
		<p class="text-gray-600 p-4 bg-gray-50 rounded-md">No objects found in this namespace.</p>
	{:else}
		<div class="overflow-x-auto bg-white shadow-md rounded-lg">
			<table class="table w-full text-left">
				<thead>
					<tr class="bg-gray-100 text-gray-600 uppercase text-sm leading-normal">
						<th class="py-3 px-6">Key</th>
						<th class="py-3 px-6">Size</th>
						<th class="py-3 px-6">Content Type</th>
						<th class="py-3 px-6">Created By</th>
						<th class="py-3 px-6">Last Modified</th>
						<th class="py-3 px-6 text-center">Actions</th>
					</tr>
				</thead>
				<tbody class="text-gray-700 text-sm font-light">
					{#each objects as object (object.key)}
						<tr class="border-b border-gray-200 hover:bg-gray-50">
							<td class="py-3 px-6 whitespace-nowrap">{object.key}</td>
							<td class="py-3 px-6 whitespace-nowrap">
								{formatSize(object.metadata.size)}
							</td>
							<td class="py-3 px-6 whitespace-nowrap">
								{object.metadata.content_type}
							</td>
							<td class="py-3 px-6 whitespace-nowrap">
								{object.metadata.created_by}
							</td>
							<td class="py-3 px-6 whitespace-nowrap">
								{new Date(object.metadata.created_at).toLocaleString()}
							</td>
							<td class="py-3 px-6 text-center">
								<div class="flex item-center justify-center space-x-2">
									<button
										on:click={() => handleDownload(object.key)}
										class="btn btn-sm btn-info text-white"
										disabled={loading}
									>
										Download
									</button>
								</div>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</div>

<style lang="postcss">
	@reference "tailwindcss";

	:global(html) {
		background-color: #f3f4f6; /* Light gray background */
	}

	/* Minimal DaisyUI-like styling if DaisyUI is not fully integrated */
	.btn {
		@apply font-semibold py-2 px-4 rounded-lg cursor-pointer;
		transition: background-color 0.2s ease-in-out;
	}

	.btn-primary {
		@apply bg-blue-500 text-white;
	}

	.btn-primary:hover {
		@apply bg-blue-600;
	}

	.btn-info {
		@apply bg-sky-500 text-white;
	}

	.btn-info:hover {
		@apply bg-sky-600;
	}

	.btn-error {
		@apply bg-red-500 text-white;
	}

	.btn-error:hover {
		@apply bg-red-600;
	}

	.btn-ghost {
		@apply bg-transparent text-gray-500;
	}

	.btn-ghost:hover {
		@apply bg-gray-200;
	}

	.btn-xs {
		@apply px-2 py-1 text-xs;
	}

	.btn:disabled {
		@apply opacity-50 cursor-not-allowed;
	}

	.input,
	.file-input {
		@apply border border-gray-300 rounded-md p-2;
	}

	.input:focus,
	.file-input:focus {
		@apply outline-none ring-2 ring-blue-500/50;
	}

	.alert {
		@apply p-4 rounded-md flex items-center;
	}

	.alert-error {
		@apply bg-red-100 text-red-700 border border-red-400;
	}

	.alert-error svg {
		@apply w-5 h-5 mr-3;
	}

	/* Table specific styles for better readability */
	.table th,
	.table td {
		padding: 12px 24px;
	}

	.table thead th {
		border-bottom: 2px solid #e2e8f0; /* gray-200 */
	}
</style>