import { type CompletionResponse } from '../index.js';

export type OpenAITextPart = {
	type: 'text';
	text: string;
};

export type OpenAIImageURLPart = {
	type: 'image_url';
	image_url: {
		url: string;
		detail?: 'auto' | 'low' | 'high';
	};
};

export type OpenAIImageFilePart = {
	type: 'image_file';
	image_file: {
		file_id: string;
		detail?: 'auto' | 'low' | 'high';
	};
};

export type OpenAIInputAudioPart = {
	type: 'input_audio';
	input_audio: {
		data: string;
		format: 'wav' | 'mp3';
	};
};

export type OpenAIFilePart = {
	type: 'file';
	file: {
		file_id?: string;
		file_data?: string;
		filename?: string;
	};
};

export type OpenAIRefusalPart = {
	type: 'refusal';
	refusal: string;
};

export type OpenAIPart =
	| OpenAITextPart
	| OpenAIImageURLPart
	| OpenAIImageFilePart
	| OpenAIInputAudioPart
	| OpenAIFilePart
	| OpenAIRefusalPart;

export type OpenAIMessage = {
	id: string;
	role: 'user' | 'assistant';
	content: OpenAIPart[];
};

export type SyncCompletionResponse = CompletionResponse<OpenAIMessage>;
export type SyncResumeResponse = CompletionResponse<OpenAIMessage>;
