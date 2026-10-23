import type {
  OpenAIFilePart,
  OpenAIImageURLPart,
  OpenAIInputAudioPart,
} from "../sync/index.js";

/**
 * The content parts a `file_available` event can carry.
 *
 * Text is not among them: prose arrives through `text_delta`, and a document
 * is announced whole because there is nothing to stream.
 */
export type DocumentPart =
  | OpenAIImageURLPart
  | OpenAIInputAudioPart
  | OpenAIFilePart;

// Optional: A helper type to strictly represent serde_json::Value.
// You can replace this with `any` or `unknown` depending on your TS strictness preferences.
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

export type SSEvent =
  | { type: "error"; error: string }
  | { type: "start"; message_id: string }
  | { type: "text_start"; id: string }
  | { type: "text_delta"; id: string; delta: string }
  | { type: "text_end"; id: string }
  | { type: "reasoning_start"; id: string }
  | { type: "reasoning_delta"; id: string; delta: string }
  | { type: "reasoning_end"; id: string }
  | {
      type: "tool_input_start";
      tool_call_id: string;
      tool_name: string;
      /** What the tool is (`tavily-web-search`, `workspace`,
       *  `code-execution`, …): what to show the call as. */
      tool_type?: string;
      /** The group it belongs to: the MCP server or the definition that
       *  expanded into it. */
      bundle?: string;
    }
  | {
      type: "tool_input_available";
      tool_call_id: string;
      tool_name: string;
      input: JsonValue;
    }
  | { type: "tool_output_available"; tool_call_id: string; output: JsonValue }
  | { type: "start_step"; id: string }
  | { type: "finish" }
  | {
      type: "request_feedback";
      tool_call_id: string;
      node_id: string;
      tool_name: string;
      arguments: JsonValue;
      tool_type?: string;
      bundle?: string;
    }
  | { type: "still_suspended" }
  | {
      /**
       * A document that was used as context for the model. Emitted while
       * the search runs, so these arrive before the text they support.
       */
      type: "reference";
      title: string;
      url: string;
      /** A preview of the document, with `<b>` hit highlighting. */
      snippet: string | null;
      /** Verbatim extracts from the document that back the answer. */
      extractive_answer: string[] | null;
    }
  | {
      /** How well the model's response is grounded in its references. */
      type: "grounding_score";
      score: number;
    }
  | {
      /**
       * A document produced during the turn: an image a tool generated, a
       * file it fetched, or media the model itself returned.
       *
       * Unlike `reference`, this belongs *in* the message's content — the
       * agent keeps it as part of the assistant turn, so the model can be
       * asked to revise what it made.
       */
      type: "file_available";
      /** Groups the file with the message being built, as `text_delta` does. */
      id: string;
      part: DocumentPart;
    };
