# Authenticating to LLM Providers

This is a work in progress as, more than authenticating to LLMs themselves, the approach is to authenticate to their provider. 

## API Keys

Some providers require API Keys. When this is the case, your API key should be available as an environment variable for Forma to look up. The name of the variable is specified [on their own docs](../reference_docs/auto-genaiclient.md).

## Google Cloud (Vertex AI)

The `vertex-ai` provider does not use API Keys. Instead, it uses Google Cloud's [Application Default Credentials](https://cloud.google.com/docs/authentication/application-default-credentials). During development, you get them by logging in with the `gcloud` CLI:

```sh
gcloud auth application-default login
```

And you also need to tell Forma which Google Cloud project to use, with the `GOOGLE_CLOUD_PROJECT` variable (e.g., in your `.env` file). In production (e.g., in Cloud Run), the credentials of the service account running your agent are used instead. Check the details [on their own docs](../reference_docs/auto-vertexai.md).