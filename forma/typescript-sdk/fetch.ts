import {
	RESUME_PATH,
	USER_ID_HEADER,
	SESSION_ID_HEADER,
	type ResumeResponse,
	type ResumeRequest,
	CHAT_PATH,
	INIT_SESSION_PATH,
	TIME_ZONE_HEADER
} from './index.js';

function authHeaders(token?: string): Record<string, string> {
	return token ? { Authorization: `Bearer ${token}` } : {};
}

// The device's time zone, so the agent's current-date tool can tell the user
// their own date rather than the server's. Left out where the runtime cannot
// say (very old browsers, some server-side contexts).
function timeZoneHeader(): Record<string, string> {
	try {
		const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
		return zone ? { [TIME_ZONE_HEADER]: zone } : {};
	} catch {
		return {};
	}
}

export async function* resume_stream<T>({
	userId,
	sessionId,
	request,
	endpoint,
	token
}: {
	userId?: string;
	sessionId?: string;
	endpoint?: string;
	request: ResumeRequest;
	token?: string;
}): AsyncGenerator<T> {
	const response = await fetch(endpoint || RESUME_PATH, {
		method: 'POST',
		headers: {
			'Content-Type': 'application/json',
			Accept: 'text/event-stream',
			[USER_ID_HEADER]: userId || '',
			[SESSION_ID_HEADER]: sessionId || '',
			...timeZoneHeader(),
			...authHeaders(token)
		},
		body: JSON.stringify(request)
	});

	const reader = response.body!.getReader();
	const decoder = new TextDecoder();
	let buffer = '';

	while (true) {
		const { done, value } = await reader.read();
		if (done) break;

		buffer += decoder.decode(value, { stream: true });
		const parts = buffer.split('\n\n');
		buffer = parts.pop()!;

		for (const part of parts) {
			const dataLine = part.split('\n').find((l) => l.startsWith('data:'));
			if (dataLine) {
				yield JSON.parse(dataLine.slice('data:'.length).trim()) as T;
			}
		}
	}
}

export async function* stream<I, O>({
	userId,
	sessionId,
	message,
	endpoint,
	token
}: {
	userId?: string;
	sessionId?: string;
	message: I;
	endpoint?: string;
	token?: string;
}): AsyncGenerator<O> {
	const response = await fetch(endpoint || CHAT_PATH, {
		method: 'POST',
		headers: {
			'Content-Type': 'application/json',
			Accept: 'text/event-stream',
			[USER_ID_HEADER]: userId || '',
			[SESSION_ID_HEADER]: sessionId || '',
			...timeZoneHeader(),
			...authHeaders(token)
		},
		body: JSON.stringify(message)
	});

	const reader = response.body!.getReader();
	const decoder = new TextDecoder();
	let buffer = '';

	while (true) {
		const { done, value } = await reader.read();
		if (done) break;

		buffer += decoder.decode(value, { stream: true });
		const parts = buffer.split('\n\n');
		buffer = parts.pop()!;

		for (const part of parts) {
			const dataLine = part.split('\n').find((l) => l.startsWith('data:'));
			if (dataLine) {
				yield JSON.parse(dataLine.slice('data:'.length).trim()) as O;
			}
		}
	}
}

export async function chat<I, O>({
	message,
	userId,
	sessionId,
	endpoint,
	token
}: {
	message: I;
	userId?: string;
	sessionId?: string;
	endpoint?: string;
	token?: string;
}): Promise<O> {
	const response = await fetch(endpoint || CHAT_PATH, {
		method: 'POST',
		headers: {
			'Content-Type': 'application/json',
			[USER_ID_HEADER]: userId || '',
			[SESSION_ID_HEADER]: sessionId || '',
			...timeZoneHeader(),
			...authHeaders(token)
		},
		body: JSON.stringify(message)
	});
	return response.json() as O;
}

export async function resume<T>({
	userId,
	sessionId,
	request,
	endpoint,
	token
}: {
	userId?: string;
	sessionId?: string;
	request: ResumeRequest;
	endpoint?: string;
	token?: string;
}): Promise<ResumeResponse<T>> {
	const response = await fetch(endpoint || RESUME_PATH, {
		method: 'POST',
		headers: {
			'Content-Type': 'application/json',
			[USER_ID_HEADER]: userId || '',
			[SESSION_ID_HEADER]: sessionId || '',
			...timeZoneHeader(),
			...authHeaders(token)
		},
		body: JSON.stringify(request)
	});
	if (!response.ok) {
		throw new Error(await response.text());
	}
	return response.json();
}

export async function init({
	userId,
	endpoint,
	token
}: {
	userId?: string;
	endpoint?: string;
	token?: string;
}): Promise<{
	session_id: string;
}> {
	const response = await fetch(endpoint || INIT_SESSION_PATH, {
		method: 'GET',
		headers: {
			[USER_ID_HEADER]: userId || '',
			...authHeaders(token)
		}
	});
	if (!response.ok) {
		throw new Error(await response.text());
	}
	return response.json();
}

export async function forwardToAgent({
	request,
	endpoint,
	apiKey
}: {
	request: Request;
	endpoint?: string;
	apiKey?: string;
}): Promise<Response> {
	const headers = new Headers(request.headers);
	if (endpoint) {
		headers.set('Authorization', `Bearer ${apiKey}`);
	}

	return fetch(endpoint || request.url, {
		method: request.method,
		headers,
		body: request.body,
		duplex: 'half'
	} as RequestInit);
}
