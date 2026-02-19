import { json } from '@sveltejs/kit';
import jwt from 'jsonwebtoken';
import type { RequestHandler } from './$types';

const OBJECT_REGISTRY_KEY_ID = process.env.OBJECT_REGISTRY_KEY_ID;
const OBJECT_REGISTRY_PRIVATE_KEY = process.env.OBJECT_REGISTRY_PRIVATE_KEY;
const OBJECT_REGISTRY_API_URL = process.env.OBJECT_REGISTRY_API_URL || 'http://localhost:8000'; // Default to localhost

if (!OBJECT_REGISTRY_KEY_ID || !OBJECT_REGISTRY_PRIVATE_KEY) {
	console.warn(
		'WARNING: OBJECT_REGISTRY_KEY_ID or OBJECT_REGISTRY_PRIVATE_KEY not set. API calls will fail.'
	);
}

async function generateJwtToken(): Promise<string> {
	if (!OBJECT_REGISTRY_KEY_ID || !OBJECT_REGISTRY_PRIVATE_KEY) {
		throw new Error('Missing OBJECT_REGISTRY_KEY_ID or OBJECT_REGISTRY_PRIVATE_KEY environment variables.');
	}

	const now = Math.floor(Date.now() / 1000); // Current timestamp in seconds
	const expiration = now + 60 * 15; // Token expires in 15 minutes

	const payload = {
		iat: now,
		exp: expiration,
		aud: 'object-registry',
		iss: 'object-registry-ui', // Or a more specific issuer name
		sub: 'object-registry'
	};

	const header = {
		alg: 'RS256',
		kid: OBJECT_REGISTRY_KEY_ID
	};

	// Sign the token using the private key
	const token = jwt.sign(payload, OBJECT_REGISTRY_PRIVATE_KEY, { algorithm: 'RS256', header });
	return token;
}

const proxyRequest = async ({ request, params, url: eventUrl }: RequestHandler): Promise<Response> => {
	const method = request.method;
	const targetPath = params.path;
	const url = new URL(`${OBJECT_REGISTRY_API_URL}/${targetPath}${eventUrl.search}`);

	try {
		const token = await generateJwtToken();

		console.log(`[Proxy Request] ${method} ${url.toString()}`);

		const headers = new Headers(request.headers);
		headers.set('Authorization', `Bearer ${token}`);
		headers.delete('host'); // Remove host header to prevent issues with proxying

		let body: BodyInit | null = null;

		// For methods that typically have a body, clone the request and get the body
		if (method !== 'GET' && method !== 'HEAD') {
			try {
				// Attempt to parse as JSON first, then fallback to text
				const requestBody = await request.clone().json();
				body = JSON.stringify(requestBody);
				headers.set('Content-Type', 'application/json');
			} catch {
				// If not JSON, try text
				body = await request.clone().text();
			}
		}

		const apiResponse = await fetch(url.toString(), {
			method,
			headers,
			body,
			// Disable follow redirect for PUT requests to avoid issues with S3 pre-signed URLs
			redirect: method === 'PUT' ? 'manual' : 'follow'
		});

		console.log(`[Proxy Response] ${apiResponse.status} ${apiResponse.statusText} for ${method} ${url.toString()}`);

		// If the API returns a redirect (e.g., S3 pre-signed URL for PUT),
		// we should just pass that redirect response back to the client.
		if (apiResponse.status >= 300 && apiResponse.status < 400 && method === 'PUT') {
			return apiResponse;
		}

		const responseHeaders = new Headers(apiResponse.headers);
		// Remove content-encoding if present, as it might cause issues with proxying
		responseHeaders.delete('content-encoding');
		// Remove transfer-encoding if present
		responseHeaders.delete('transfer-encoding');

		return new Response(apiResponse.body, {
			status: apiResponse.status,
			statusText: apiResponse.statusText,
			headers: responseHeaders
		});
	} catch (error: any) {
		console.error('Proxy error:', error);
		return json({ error: error.message }, { status: 500 });
	}
};

export const GET: RequestHandler = proxyRequest;
export const POST: RequestHandler = proxyRequest;
export const PUT: RequestHandler = proxyRequest;
export const DELETE: RequestHandler = proxyRequest;
export const PATCH: RequestHandler = proxyRequest;
export const OPTIONS: RequestHandler = proxyRequest;
export const HEAD: RequestHandler = proxyRequest;