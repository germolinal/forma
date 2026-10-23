/**
 * The response returned after calling the /v1/chat API,
 * indicating that the agent completed the request succesfully
 */
export type CompletedResponse<O> = {
	status: 'completed';
	data: O;
};

/**
 * Data received when the AI Agent is looking
 * for external feedback.
 */
export type SuspendedData = {
	/**
	 * The ID of the tool call
	 */
	tool_call_id: string;
	/**
	 * The node that emitted the feedback request
	 */
	node_id: string;
	/**
	 * Tha id of the tool that the node wants to call
	 */
	tool_name: string;
	/**
	 * The arguments it wants to use to call the tool.
	 */
	arguments: Record<string, unknown>;
};

/**
 * Data used for submitting feedback and
 * confirming tool usage
 */
export type ResumeRequest = {
	/**
	 * The tool call ID to resume, as provided by the AI Agent
	 */
	tool_call_id: string;
	/**
	 * Whether the tool call should be rejected
	 */
	reject: boolean;
	/**
	 * The arguments to pass to the tool, if not rejected.
	 * These do not have to be the same as the ones provided by the node,
	 * allowing the user to modify them.
	 */
	arguments?: Record<string, unknown>;

	/**
	 * The result of a client-side ran tool.
	 */
	result?: Record<string, unknown>;
};

/**
 * The response returned after calling the /v1/chat API,
 * indicating that the agent is still processing the request
 */
export type SuspendedResponse = {
	status: 'suspended';
	data: SuspendedData;
};

/**
 * The response returned after calling the /v1/chat API,
 * indicating that the completion is either complete or pending
 */
export type CompletionResponse<O> = CompletedResponse<O> | SuspendedResponse;

/**
 * The response returned after calling the /v1/resume API,
 * indicating that the completion is either complete or pending.
 * A rejected call completes like any other: the agent is told
 * the user rejected it, and answers. `suspended` means the resumed
 * agent stopped again, at another call that needs an answer;
 * `still_suspended`, that other calls of the same turn are waiting.
 */
export type ResumeResponse<O> =
	| CompletedResponse<O>
	| SuspendedResponse
	| { status: 'still_suspended' };

/**
 * The response returned when initiating a new session
 */
export type InitResponse = {
	session_id: string;
};
