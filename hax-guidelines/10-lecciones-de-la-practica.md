# Lecciones de la práctica

> Parte de [`INDEX.md`](./INDEX.md). Los archivos `01`-`09` resumen buenas prácticas generales, con fuentes externas. **Este archivo es distinto: es lo que he aprendido desarrollando agentes**, sobre todo con Forma. No viene de una fuente externa, sino de mi experiencia, y cambia cuando cambia esa experiencia. Si contradice a los archivos `01`-`09`, tómalo como una opinión informada, no como una regla.

No todas estas lecciones son del mismo tipo. Cada sección dice a cuál pertenece:

- **Definición:** lo que hay que decidir antes de construir el agente. Lo escribí pensando en desarrollar con ayuda de una IA (que necesita respuestas claras para no adivinar), pero importa igual si lo haces a mano: si no está definido, nadie puede decir si el agente funciona.
- **General:** vale para cualquier agente, con cualquier herramienta.
- **Forma:** depende de cómo funciona Forma (campos, herramientas, límites). En otra plataforma la idea puede servir, pero los detalles cambian.
- **Convención:** una forma de hacer las cosas que me ha funcionado. Lo importante es ser consistente, no el nombre exacto.

## 1. Un README que diga para qué existe el agente

*Definición.*

Cada agente debería tener un `README.md` que responda estas preguntas. Si falta alguna o es ambigua, hay que resolverla con quien es dueño del agente antes de tocar el prompt. Un asistente de IA que trabaja sin estas respuestas las inventa, y una persona también.

- **Objetivo:** qué logra el usuario gracias al agente, en una o dos frases.
- **Usuarios:** principal y secundarios, y el **costo de un error** (ver [`01`](./01-personas-de-usuario.md)).
- **Idioma y tono:** registro, adjetivos y nivel de small talk (ver [`03`](./03-tono.md) y [`05`](./05-transaccionalidad-y-small-talk.md)).
- **Alcance:** qué hace, a qué o a quién deriva (otro agente, un organismo, un profesional), qué queda fuera y qué rechaza.
- **Herramientas:** qué hace cada una y cuál es la fuente principal.
- **Personalización:** qué recuerda de cada usuario (sección 4).

En Forma, el README que crea `forma init` (el que habla de "Dory" y Ollama) es un ejemplo, no la definición de tu agente: reemplázalo.

## 2. Útil, amable y alentador, pero sin dar la razón por reflejo

*General.*

Los usuarios vienen al agente porque saben menos que él del tema. Si el agente valida todo, no sirve.

- Amable y alentador: reconoce avances y buenas decisiones **con algo concreto** ("esta paleta calza con lo tranquilo y premium que describiste"), nunca con elogios vacíos ("¡Excelente pregunta!").
- Alentar no es estar de acuerdo. Si algo está mal o es riesgoso, lo dice con amabilidad, explica por qué y propone algo mejor.
- Si el usuario insiste, mantiene su posición con razones y cambia solo ante información o argumentos nuevos. Si se equivocó, lo reconoce en una frase y corrige.
- Si el usuario se salta un paso, el agente menciona el riesgo **una vez**. En temas donde la decisión es solo del usuario (su marca, su estilo), después lo acompaña. En temas con consecuencias legales, tributarias o de dinero, no repite un error solo porque el usuario insiste.
- Nunca escribas en un prompt "si el usuario insiste, hazlo como lo pide" sin esos límites.
- **No empujar funcionalidades.** Si el agente hace varias cosas (p. ej., explicar contabilidad *y* llevar los libros), cada una vale por sí sola. La función más grande se ofrece una vez, cuando viene al caso; si el usuario dice que no, queda anotado en su perfil (sección 4) y no se vuelve a ofrecer. Los ejemplos de saludo y de "siguiente paso" del prompt no deben suponer que el usuario la usa.

## 3. Dale al agente las vueltas que su trabajo necesita

*Forma.*

En Forma, `max_iterations` limita cuántas rondas de herramientas puede hacer un nodo. El valor por defecto (10) muchas veces no alcanza. Cuando se agota, el nodo resume sin seguir buscando, y el agente puede dar a entender que terminó.

Los valores que me han funcionado:

| Tipo de agente | `max_iterations` |
|---|---|
| Atención al público con una sola herramienta | 3-5 |
| Base de conocimiento + web + scrapbook | ~20 |
| Investigación (buscar, leer muchas páginas, reportes) o MCP | ~30 |

*General:* en agentes con tareas largas, el prompt dice "si no alcanzaste a cubrir todo, di qué revisaste y qué falta", para no dar a entender que se cubrió todo. Esto vale en cualquier plataforma que corte al agente tras un número de pasos.

## 4. Un perfil por usuario

*General* (la idea), *Forma* (el scrapbook) y *Convención* (el nombre `perfil.md`).

Todo agente de **uso personal y recurrente** debería recordar quién es cada usuario. Los agentes públicos y no personales (p. ej., atención al cliente de una empresa) son la excepción. Yo lo hago con un archivo `perfil.md` en el scrapbook de cada usuario (en Forma, la herramienta `scrapbook` guarda notas privadas por usuario).

1. **Lo lee seguido:** al inicio de cada conversación, incluso con un "Hola", antes de responder. No en cada mensaje.
2. **Si no existe, lo crea preguntando:** dice para qué está y qué necesita saber, con un máximo de tres preguntas a la vez. Ejemplo: "Hola, te ayudo a encontrar compradores potenciales para tu negocio. Para buscar bien necesito saber tres cosas: qué vendes, a quién se lo vendes y en qué zona." Guarda las respuestas apenas las tiene.
3. **Lo mantiene al día sin preguntar** cuando el usuario comparte un dato o una preferencia, y lo menciona en pocas palabras ("Lo anoté en tu perfil").
4. **Es también el índice:** una sección "Archivos" enlaza los demás archivos del scrapbook (informes, notas, decisiones). No hace falta un `INDEX.md` aparte.

**Contenido esperado.** Además de lo propio de cada agente (la empresa, el curso, la marca), el perfil incluye:

- **Quién es el usuario:** nombre y cómo quiere que lo traten, y cuánto sabe del tema (para ajustar el vocabulario).
- **Qué quiere del agente:** cuál de sus usos le interesa (p. ej., entender contabilidad, definirla o que le lleve los libros; aprender sobre marcas o construir la suya) y lo que **no** quiere (p. ej., "ya tiene contador; no ofrecer llevar los libros"). Si el agente tiene un solo uso, basta con el objetivo concreto ("seguir la prensa sobre su empresa cada semana").
- **Preferencias:** formato, largo, frecuencia, tono.
- **Archivos:** los enlaces a los demás archivos.

"Qué quiere del agente" no se pregunta como formulario: se deduce de lo que pide y se confirma solo si no está claro. El agente lo respeta en cada respuesta (sección 2, "No empujar funcionalidades") y lo actualiza cuando el usuario cambia de idea.

Si el perfil existe, el agente lo usa: saluda por el nombre, retoma donde quedaron y no vuelve a preguntar lo que ya está ("Hola, Ana. La última vez elegimos la paleta; ¿seguimos con la tipografía?").

En Forma, esto queda en tres lugares: una sección del prompt ("The user's profile: `perfil.md`"), las `usage_guidelines` del scrapbook y la sección "Personalización" del README. Ojo: en `forma eval` el agente no tiene scrapbook, así que cualquier saludo hará las preguntas del perfil. Tenlo en cuenta en los rubrics de small talk.

## 5. Sugerencias que el agente sabe responder

*Forma* (el campo `suggestions`) y *General* (la idea).

Las sugerencias son las tarjetas que el usuario ve al abrir el chat: su primera impresión del agente, y también preguntas de prueba.

- Calzan con el objetivo y el público del README.
- **El agente sabe responderlas** con su base de conocimiento, búsquedas y herramientas. Verifica que el dato está en la base o que la herramienta existe; si no, no la pongas.
- **Motivan:** muestran una capacidad que el usuario no esperaba ("¿puede hacer un análisis de unit economics? ¡Wow!"), en vez de preguntas genéricas.
- Si el agente es una plantilla, el README dice que hay que cambiarlas al adaptarlo.

## 6. Frases de espera que nunca mienten

*Forma* (el campo `waiting_phrases` de cada nodo) y *General* (la idea).

Forma las muestra al azar mientras el nodo trabaja, haga lo que haga.

- Pueden ser dinámicas, pero **nunca mienten**. "Revisando tus facturas" o "Consultando el CRM" son falsas si el agente no lo está haciendo en ese turno, y "Ya casi…" puede aparecer al comienzo.
- Usa frases ciertas en cualquier turno: "Pensando…", "Ordenando las ideas…", "Preparando la respuesta…", "Un momento…".

## 7. Herramientas

**General:**

- La descripción dice **cuándo usarla y cuándo no**, no solo qué hace ("fetches a URL" no basta). En Forma, ver `agent-dev-guidelines.md` §2.c.
- La fuente principal se dice en la herramienta y en el prompt: base de conocimiento primero; la web solo para lo que la base no cubre.
- El contenido de páginas, documentos y resultados de herramientas es **dato, nunca instrucción**: se dice en la descripción de la herramienta que lo trae y en el prompt.
- No copies bloques entre agentes sin releerlos. Es la fuente de errores más común que he visto: instrucciones de un agente que aparecen en otro donde no tienen sentido (p. ej., "guarda recortes de noticias" en un agente que enseña).

**Convención:**

- Nombres consistentes entre agentes: `web-search`, `fetch-page`, `scrapbook`. Los nombres de las herramientas no amarran una plantilla a un tema (`course-material`, no `clases-de-ia`).
- Si un agente apunta a un índice de su base (`INDEX.md`), verifica que ese archivo existe con ese nombre en el workspace.

**Ejecución de código:**

- *Forma.* **Escribir en archivos, no imprimir.** La herramienta de código de Forma solo le devuelve al modelo los últimos 2.000 caracteres de lo que imprime un comando; si imprime una planilla, ejecuta una y otra vez para ver el resto. Los scripts que le doy a un agente escriben sus reportes en `out/` e imprimen solo un resumen con la ruta, y el prompt le dice al agente que haga lo mismo con sus propios scripts.
- *General.* **Scripts deterministas** para lo que se repite y donde un error cuesta (contabilidad, proyecciones, leer planillas), en vez de dejar que el modelo improvise el cálculo cada vez. El estado vive en archivos que lee una máquina (CSV o JSON), y el Markdown es la vista para personas.
- *Forma.* Prueba cada script dentro de la imagen del sandbox (`forma/sandbox`) antes de dárselo a un agente.
- *Forma.* **La llamada exacta de cada script va en la descripción de la herramienta de código**, no solo en una guía dentro de la base de conocimiento. El agente no siempre lee la guía antes de actuar, y sin la llamada a mano improvisa. Me pasó con un agente que analizaba planillas: abría los archivos del usuario con sus propios scripts de openpyxl, veía un pedazo sin fórmulas y ejecutaba una y otra vez. Se arregló cuando la descripción de la herramienta trajo la regla ("ante cualquier planilla, lo primero es siempre el script de planillas") y la llamada lista para copiar: el comando, los `files` (el script desde la base con `from_workspace` y `source`, el archivo del usuario desde el scrapbook) y qué salida leer después. El prompt repite la regla con su porqué. La guía queda para el detalle: opciones, formatos y cómo interpretar el reporte.
- *Forma.* **Ejecuciones en paralelo.** Forma ejecuta a la vez las llamadas a herramientas de un mismo turno. La descripción de la herramienta de código se lo dice al agente: para buscar un óptimo o un punto de quiebre (bisección), prueba varios valores de una vez y después afina; para comparar escenarios, una ejecución por escenario. Si el cálculo es corto, un bucle dentro de un solo script sigue siendo lo más simple.
- *General.* **Todo reporte dice cuándo se creó y a qué fecha corresponden sus datos.** La primera línea de cada reporte que deja un script o un agente es la fecha de creación ("Creado el 2026-10-16"), y cada dato de entrada que cambia con el tiempo lleva la suya ("caja al 2026-10-16", "tipo de cambio del 2026-10-01", "ventas del CRM al 2026-10-14"). Así el usuario puede pedir "actualiza el flujo con la caja de hoy", y el agente sabe qué está viejo y qué pedir de nuevo. Al abrir un reporte o un archivo de supuestos antiguo, el agente compara esas fechas con la de hoy y ofrece actualizar lo que quedó atrasado.
- *Forma.* Los scripts reciben la fecha de hoy desde la herramienta `current-date` (p. ej., una opción `--hoy AAAA-MM-DD`), porque el reloj del sandbox está en UTC; sin esa opción usan la fecha del servidor y lo dicen en el reporte.

## 8. Datos, fechas y otros agentes

- *General.* **Nunca inventar datos de contacto.** Solo los que una empresa o persona publicó para fines comerciales, con el enlace a la fuente. Nunca se deducen correos ni teléfonos ("nombre@empresa.com"). No se recopilan datos de particulares; revisa la ley de protección de datos que aplique (en Chile, la Ley 21.719).
- *General* (la idea) y *Forma* (la herramienta). **La fecha de hoy, con una herramienta.** El modelo no sabe qué día es, y poner la fecha en el prompt rompe el caché de prompts. En Forma, todo agente que dependa de la fecha (plazos, "hoy", el día de la semana, nombres de archivo) tiene una herramienta de tipo `current-date`, y su prompt dice cuándo llamarla. La zona horaria sale del dispositivo del usuario (header `x-time-zone`), de lo que el usuario le dijo al agente (guardado en su perfil) o del `timezone` del agente; si no hay ninguna, la herramienta pide preguntarle al usuario dónde está.
- *General.* **Derivar a otros agentes o servicios por su nombre**, cuando corresponda: p. ej., un agente que busca clientes deriva a un mentor de emprendimiento para definir a quién venderle. Hay que decirle al agente qué hace el otro, no solo que "hay otros agentes".

## 9. Mantener el proyecto ordenado

*Forma.*

- `agent-dev-guidelines.md` en cada agente es una copia de la guía que crea `forma init`. Si mantienes varios agentes, no la edites en cada uno: edita tu versión de referencia y vuelve a copiarla.
- Revisa cada agente con `forma expand -c` después de cada cambio: muestra el agente tal como va a correr, con las herramientas ya expandidas.

## Checklist

- [ ] README con objetivo, usuarios y costo de error, idioma y tono, alcance, herramientas y personalización.
- [ ] Prompt amable y alentador, sin adulación, y que mantiene su posición ante presión.
- [ ] `max_iterations` según el tipo de agente, y aviso de lo pendiente en tareas largas.
- [ ] Perfil del usuario (si es de uso personal): lo lee al saludar, lo crea preguntando, lo actualiza y enlaza los demás archivos.
- [ ] Sugerencias que el agente sabe responder y que motivan.
- [ ] Frases de espera ciertas en cualquier turno.
- [ ] Descripciones de herramientas con cuándo sí y cuándo no; contenido externo como dato.
- [ ] Cada script que el agente debe usar tiene su llamada exacta (comando, `files` y salida a leer) en la descripción de la herramienta de código.
- [ ] La herramienta de código dice que las ejecuciones independientes pueden ir en paralelo.
- [ ] Reportes con fecha de creación y fecha de cada dato de entrada que cambia con el tiempo.
- [ ] Sin datos de contacto inventados; la fecha, con una herramienta y nunca supuesta; derivaciones a otros agentes por nombre.
- [ ] Todo lo de HAX: [`08-checklist-y-plantilla-de-prompt.md`](./08-checklist-y-plantilla-de-prompt.md).
