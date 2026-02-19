// src/lib/api.ts
const BASE_URL = '/api/object-registry'; // Our SvelteKit proxy endpoint

export interface MetadataResponse {
	namespace: string;
	checksum: string;
	size: number;
	content_type: string;
	created_by: string;
	created_at: string;
	version: string;
	labels: Record<string, string>;
}

export interface ObjectMetadata {
	key: string;
	metadata: MetadataResponse;
}

export interface ListObjectsResponse {
	objects: ObjectMetadata[];
}

export interface ObjectResponse {
	key: string;
	is_base64_encoded: boolean;
	payload: any; // Can be JSON, YAML, or Base64 string
	metadata: MetadataResponse;
}

async function handleError(response: Response, action: string): Promise<never> {
	let message = `Failed to ${action}: ${response.statusText}`;
	try {
		const contentType = response.headers.get('content-type');
		if (contentType && contentType.includes('application/json')) {
			const errorBody = await response.json();
			message = errorBody.error || errorBody.message || JSON.stringify(errorBody);
		} else {
			const textBody = await response.text();
			if (textBody) {
				message = textBody;
			}
		}
	} catch (e) {
		// If parsing fails, stick with the default message
	}
	throw new Error(message);
}

export async function listObjects(namespace: string): Promise<ObjectMetadata[]> {
	const response = await fetch(`${BASE_URL}/${namespace}`);
	if (!response.ok) {
		await handleError(response, 'list objects');
	}
	const data: ListObjectsResponse = await response.json();
	return data.objects;
}

export async function listNamespaces(): Promise<string[]> {
	const response = await fetch(`${BASE_URL}/namespaces`);
	if (!response.ok) {
		await handleError(response, 'list namespaces');
	}
	return await response.json();
}

export async function downloadObject(namespace: string, key: string): Promise<{ blob: Blob; filename: string }> {
	const response = await fetch(`${BASE_URL}/${namespace}/${key}`);
	if (!response.ok) {
		await handleError(response, 'download object');
	}

	const data: ObjectResponse = await response.json();
	let blob: Blob;

	if (data.is_base64_encoded) {
		const binaryString = window.atob(data.payload);
		const bytes = new Uint8Array(binaryString.length);
		for (let i = 0; i < binaryString.length; i++) {
			bytes[i] = binaryString.charCodeAt(i);
		}
		blob = new Blob([bytes], { type: data.metadata.content_type });
	} else if (typeof data.payload === 'object') {
		blob = new Blob([JSON.stringify(data.payload, null, 2)], { type: 'application/json' });
	} else {
		blob = new Blob([data.payload], { type: data.metadata.content_type });
	}

	const filename = key.split('/').pop() || 'download';
	return { blob, filename };
}

export async function uploadObject(namespace: string, key: string, file: File): Promise<void> {
	const response = await fetch(`${BASE_URL}/${namespace}/${key}`, {
		method: 'PUT',
		body: file,
		headers: {
			'Content-Type': file.type || 'application/octet-stream'
		}
	});

	if (!response.ok) {
		await handleError(response, 'upload object');
	}
}

export async function deleteObject(namespace: string, key: string): Promise<void> {
	const response = await fetch(`${BASE_URL}/${namespace}/${key}`, {
		method: 'DELETE'
	});

	if (!response.ok) {
		await handleError(response, 'delete object');
	}
}

export interface NotifyResponse {
	type: string;
	method: string;
	urls: string[];
}

export interface EventResponse {
	namespace: string;
	id: string;
	keys: string[];
	notify: NotifyResponse;
	created_at: string;
}

export async function listEvents(namespace: string): Promise<EventResponse[]> {
	const response = await fetch(`${BASE_URL}/events/${namespace}`);
	if (!response.ok) {
		await handleError(response, 'list events');
	}
	return await response.json();
}