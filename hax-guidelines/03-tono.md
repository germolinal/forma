# Tono

> Parte de [`INDEX.md`](./INDEX.md). Cómo debe sonar el agente según el uso.

Todo agente tiene una personalidad, la defina uno o no: viene del modelo y del prompt. Como dice Shape of AI, "todo modelo viene con una personalidad por defecto", e incluso los modelos "neutros" tienen tendencias de estilo que influyen en la confianza del usuario ([Shape of AI](https://www.shapeof.ai/patterns/personality)). Es mejor **definirla a propósito** que heredarla.

## Cómo definir el tono

Describe el tono con **3-4 adjetivos + un contraste** ("cercano pero no confianzudo") y **2-3 ejemplos** de frases buenas y malas. Los adjetivos solos son ambiguos; los ejemplos anclan al modelo. Incluye:
- Registro (formal/informal; tú/usted; regionalismos).
- Nivel de calidez y de humor (por defecto, poco).
- Largo típico y uso de emojis.
- Cómo reacciona ante errores, enojo y temas sensibles.

## Tono según el uso

| Uso | Tono recomendado | Ojo con |
|---|---|---|
| **Soporte al cliente** | Empático, calmado, resolutivo; poca fórmula ("lamentamos las molestias"). Disculpa solo por errores reales. | Bucles, promesas que no se pueden cumplir, no ofrecer humano. |
| **Asesoría profesional** (contable, legal, salud, finanzas) | Sobrio, preciso, honesto sobre límites; sin jerga innecesaria. | Falsa certeza; no citar fuentes; no decir cuándo consultar a un profesional. |
| **Productividad / herramientas de trabajo** | Eficiente, directo, mínimo relleno. | Preguntas de confirmación en exceso; explicar lo obvio. |
| **Educación / onboarding** | Paciente, alentador, concreto, con ejemplos. | Condescendencia; elogio vacío. |
| **Creatividad / lluvia de ideas** | Más exploratorio y juguetón, con opiniones propias. | Que todo "suene genial": entrega crítica útil. |
| **Consumo masivo / marca** | Alineado a la voz de la marca, cálido. | Que la personalidad tape la utilidad. |
| **Temas emocionales o sensibles** | Calmado, humano, prosa (sin listas), sin diagnosticar; derivar cuando corresponde. | Adulación, dramatismo, dependencia. |
| **Agente que actúa (herramientas, dinero, envíos)** | Neutro y preciso; confirma lo irreversible. | Tono "amigable" que oculta el riesgo de la acción. |

OpenAI propone cuatro personalidades base como punto de partida: *profesional* (formal, precisa), *eficiente* (concisa y llana), *basada en hechos* (directa, con evidencia y próximos pasos claros) y *exploratoria* (entusiasta, explicativa) ([OpenAI Cookbook](https://developers.openai.com/cookbook/examples/gpt-5/prompt_personalities)).

## Ejemplo: español de Chile

Formal pero cercano, tuteo, claro y transparente; sin palabras rimbombantes. Evita modismos muy locales que no todos entienden y traducciones literales del inglés ("Espero que este mensaje te encuentre bien").

## Calidez ≠ adulación

- **Calidez** es tratar bien a la persona. **Adulación (sycophancy)** es decirle lo que quiere oír en vez de lo verdadero o útil ([Anthropic Academy](https://academy.claude.com/tutorials/what-is-sycophancy-in-ai-models)).
- La adulación puede subir el engagement, pero erosiona la confianza y la autonomía del usuario, y es más frecuente en temas personales. En conversaciones de claude.ai de marzo y abril de 2026, Anthropic encontró conducta aduladora en el 9 % de las conversaciones de orientación personal, el **25 %** en las de relaciones y el **38 %** en las de espiritualidad; la tasa se duplicó (de 9 % a 18 %) cuando el usuario discutió la primera respuesta ([Anthropic, 30 de abril de 2026](https://www.anthropic.com/research/claude-personal-guidance)).
- Reglas: no cambies una respuesta correcta solo porque el usuario insiste; sí cámbiala si aporta un argumento o dato nuevo. Da crítica concreta y accionable. Separa el reconocimiento ("entiendo que esto frustra") del contenido factual.
- Evita elogiar la pregunta ("¡Excelente pregunta!"), abrir con muletillas ("¡Claro!", "¡Por supuesto!") y validar por reflejo.
- Cálido *y* honesto a la vez. El system prompt de Claude describe bien el equilibrio: un tono cálido que evita suposiciones negativas o condescendientes sobre las capacidades del usuario, pero que "sigue dispuesto a contradecirlo y ser honesto", de forma constructiva ([Opus 4.7, abril 2026](https://platform.claude.com/docs/en/release-notes/system-prompts/claude-opus-4-7)).

## Frases y hábitos a evitar

- Preámbulos y cierres huecos ("Espero haberte ayudado", "No dudes en consultarme").
- Disculpas repetidas; una basta, y solo si hubo error.
- "Como modelo de lenguaje…" salvo que sea relevante.
- Humor forzado, exclamaciones y emojis en exceso. Claude, por ejemplo, tiene la instrucción de no usar emojis salvo que el usuario los use.
- Fingir emociones o experiencias humanas ("me encanta", "yo también lo he vivido"). Si el usuario pregunta si es una IA, responde que sí.

## Consistencia y adaptación

- Mantén el mismo tono durante la conversación y entre canales; cambios bruscos parecen fallas.
- Permite ajustes del usuario ("háblame más simple", "menos formal") y respétalos.
- El tono se **espeja parcialmente**: si el usuario es breve y técnico, sé breve y técnico; si está molesto, baja el ritmo y sé claro, no imites el enojo.
- Si cambias de modelo o de personalidad, avísalo: los cambios silenciosos desconciertan (G14 y G18 en [`06`](./06-errores-incertidumbre-y-confianza.md)).

## Cómo probar el tono

Corre las mismas 10 conversaciones (novato, apurado, molesto, error del agente, tema sensible, presión para darle la razón) y revisa: ¿suena a la marca? ¿se mantiene bajo presión? ¿alguien lo leería como servil o frío?
