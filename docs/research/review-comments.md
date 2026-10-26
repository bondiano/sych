# Что пишут senior-ревьюеры: таксономия, частоты, nits

Для правил discovery и eval-кейсов. Числа литературы и нашей выборки разделены.

## Источники

- Bacchelli, Bird. [Expectations, Outcomes, and Challenges of Modern Code Review](https://sback.it/publications/icse2013.pdf), ICSE 2013 (Microsoft, card sort 570 комментариев).
- Czerwonka, Greiler, Tilford. [Code Reviews Do Not Find Bugs](https://www.microsoft.com/en-us/research/wp-content/uploads/2015/05/PID3556473.pdf), ICSE SEIP 2015.
- Bosu, Greiler, Bird. [Characteristics of Useful Code Reviews](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/bosu2015useful.pdf), MSR 2015 (1.5M комментариев, 5 проектов).
- Sadowski et al. [Modern Code Review: A Case Study at Google](https://sback.it/publications/icse2018seip.pdf), ICSE SEIP 2018.
- Mäntylä, Lassenius. What Types of Defects Are Really Discovered in Code Reviews?, IEEE TSE 35(3), 2009, DOI 10.1109/TSE.2008.71 (paywall; числа по [слайдам](https://slidetodoc.com/what-type-of-defects-are-really-discovered-in/) и Bosu).
- Beller et al. [Modern Code Reviews in OSS: Which Problems Do They Fix?](https://flosshub.org/node/1650), MSR 2014 (по аннотации).
- Google eng-practices: [standard](https://google.github.io/eng-practices/review/reviewer/standard.html), [comments](https://google.github.io/eng-practices/review/reviewer/comments.html).
- [Conventional Comments](https://conventionalcomments.org/).

## Литература: что находят review

| Источник | Измерение |
|---|---|
| Mäntylä 2009 | ~75% находок — evolvability (documentation, visual representation, structure, organization, solution), functional ~20% (industry) / 13% (студенты); отношение от 3:1 до 5:1 |
| Beller 2014 | В двух OSS-проектах то же 75:25 maintainability:functional; 7–35% комментариев отброшены авторами |
| Bacchelli 2013 | 570 комментариев: code improvement 29% (практики 58, удаление лишнего 55, читаемость 52), defects 14% (65 из 78 — логика и corner cases) |
| Czerwonka 2015 | ~15% комментариев указывают на возможный дефект, ≥50% — сопровождаемость |
| Bosu 2015 | Полезных 64–68% во всех 5 проектах; ревьюер, уже видевший файл, — 65–71% против 32–37% у новичка; полезность падает с числом файлов |
| Sadowski 2018 | Ожидания Google: education, maintaining norms, gatekeeping, accident prevention; медиана 24 строки, 1 ревьюер, >80% изменений — не больше одной итерации |

**Полезно vs шум (Bosu, интервью авторов изменений).** Useful: functional defects, validation и corner cases
(«alternate scenarios where the current implementation may fail»), подсказки об API/дизайне/конвенциях команды.
Nit-picking (отступы, комментарии, стиль, имена, опечатки) — «somewhat useful» или «useful»: «help long-term
project maintenance». Not useful: false positives, вопросы ради понимания самого ревьюера, похвала, «работа на
будущее» и замечания к коду, который изменение не трогает. Bacchelli: формат-замечания ревьюеры делают, «because
they are easy […] but it doesn't really help».

**Google.** «There is no such thing as "perfect" code—there is only better code»; некритичное — с префиксом
«Nit:»; «On matters of style, the style guide is the absolute authority», остальной стиль — personal preference.
Метки: `Nit:`, `Optional:`, `FYI:`. Conventional Comments: `<label> [decorations]: <subject>`
(`nitpick`, `suggestion`, `issue`, `question`, …; `(non-blocking)`).

## Выборка: метод

GitHub REST `repos/{o}/{r}/pulls/comments` (новые первыми; стр. 1–3, для rust, kubernetes,
TypeScript, prometheus ещё 8–10 ради merged PR), 25.10.2026. Фильтр: top-level комментарий (не ответ), не бот,
`author_association` MEMBER/COLLABORATOR/OWNER, автор комментария ≠ автор PR, PR **merged**. Итог: **428
комментариев, 197 PR, 46 ревьюеров**: prometheus 95, react 87, TypeScript (в основном Go-порт) 77, tokio 73, rust
60, kubernetes 36. Одна основная категория на комментарий, разметка одним человеком без проверки согласия;
несколько ревьюеров дают большую долю (Darksonn, eps1lon, krajorama). Пара комментариев явно ссылается на LLM.

## Выборка: частоты

| Категория | n | % |
|---|---|---|
| Тесты | 69 | 16.1 |
| Упрощение, идиомы, переиспользование, мёртвый код | 58 | 13.6 |
| Документация, комментарии, описание PR | 51 | 11.9 |
| Корректность: логика, corner case, портируемость | 40 | 9.3 |
| Дизайн API/поведения, типы | 39 | 9.1 |
| Вопрос на понимание («Why…?», «What is this for?») | 37 | 8.6 |
| Только suggestion без текста / «Same here» | 33 | 7.7 |
| Процесс, scope, CI, похвала | 30 | 7.0 |
| Производительность, аллокации | 21 | 4.9 |
| Ошибки, логирование, assert | 18 | 4.2 |
| Именование | 11 | 2.6 |
| Конкурентность, ресурсы, lifecycle | 8 | 1.9 |
| Формат, опечатки, порядок (класс линтера) | 7 | 1.6 |
| Согласованность с конвенцией кодовой базы | 6 | 1.4 |
| Security | 0 | 0 |

Наблюдения: functional (корректность + ошибки + конкурентность) = 15.4% — совпадает с 14–15% у Bacchelli и
Czerwonka. Формата почти нет (1.6%): его снимают rustfmt/gofmt/prettier и CI. С префиксом `nit` — 35 (8.2%), и
это не только стиль: 8 docs, 5 тестов, 5 упрощений, **4 реальных бага** (непроверенный индекс, сортировка
аргумента на месте). Знак вопроса — в 42%; ```` ```suggestion ```` — в 18%; идентификатор в backticks — в 56%;
явное «not a blocker»/«follow-up» — 2.6%. Медианная длина — 140 символов.

## Как пишет senior

1. **Место + механизм + правка**, одна-две фразы. «This bypasses Tokio's coop system. We should decrement the coop
   budget when returning `None`.» Причина названа через последствие, не через вкус.
2. **Ссылка на существующее**: хелпер, соседний тест, паттерн проекта («like we do for blur», «isn't this a
   duplicate of `wrapParenDurationExpr` above?»).
3. **Вопрос вместо утверждения, когда нет уверенности**, но с конкретным сценарием: «`Nano - Scale(exponent)`
   *could* overflow, right?», «Does this work for circular references…?»
4. **Калибровка**: «nit:», «not a hard blocker», «probably not called enough to matter», «This is a pre-existing
   issue».
5. **Удалить, а не добавить**: инлайнить хелпер, убрать лишнюю проверку/else/обнуление. Добавить просят тест,
   why-комментарий, `#[must_use]`, `default` в switch — то, что ловит будущую ошибку.

### Цитаты (выборка, сокращено)

Тесты:
- «This test doesn't spawn any threads, so it's useless as a loom test.» [tokio#8505](https://github.com/tokio-rs/tokio/pull/8505#discussion_r4241590214)
- «all of your tests pass on master without your fix» [tokio#8636](https://github.com/tokio-rs/tokio/pull/8636#discussion_r4258997349)
- «we should never silence errors in tests.» [react#37710](https://github.com/react/react/pull/37710#discussion_r4246880960)
- «Let's use Jest's `spyOn` so that 'jest.restoreAllMocks' just works.» [react#37408](https://github.com/react/react/pull/37408#discussion_r3888688799)

Упрощение и идиомы:
- «This looks like it could be a `matches!`?» [rust#164744](https://github.com/rust-lang/rust/pull/164744#discussion_r4353078944)
- «Can we inline `i32::try_from(backlog).unwrap_or(i32::MAX)` directly … instead of defining a helper function and unit test?» [tokio#8511](https://github.com/tokio-rs/tokio/pull/8511#discussion_r4240972243)
- «Do we need to re-read this again? There's already an `unstable_features` before the loop, right?» [rust#163664](https://github.com/rust-lang/rust/pull/163664#discussion_r4306565573)
- «isn't this a duplicate of `wrapParenDurationExpr` above?» [prometheus#19403](https://github.com/prometheus/prometheus/pull/19403#discussion_r3773888784)
- «nit: let's avoid else here and simply return in the above line» [kubernetes#143232](https://github.com/kubernetes/kubernetes/pull/143232#discussion_r4334588961)

Документация и комментарии:
- «The comment above … says this is constant, but I think that's wrong, we do update metadata» [prometheus#20040](https://github.com/prometheus/prometheus/pull/20040#discussion_r4322173701)
- «It makes sense when you know the bug this is fixing, but future readers of this code won't know that.» [tokio#8445](https://github.com/tokio-rs/tokio/pull/8445#discussion_r4024740385)
- «Please add a comment explaining why we use a deprecated API here.» [rust#164744](https://github.com/rust-lang/rust/pull/164744#discussion_r4349780949)
- «Can you add a comment explaining where the +1 comes from.» [react#37730](https://github.com/react/react/pull/37730#discussion_r4195838826)

Именование:
- «this function name is a little misleading … maybe … invert the logic and call it `nameRequiresQu…`» [prometheus#19683](https://github.com/prometheus/prometheus/pull/19683#discussion_r4325586896)
- «nit: let's not mask imported `stats` package» [prometheus#19124](https://github.com/prometheus/prometheus/pull/19124#discussion_r3577703668)

API, типы, конвенции:
- «Please add `#[must_use]` to `FileTimes` … so that calling `times.set_modified(t);` … produces a compiler warning» [tokio#8504](https://github.com/tokio-rs/tokio/pull/8504#discussion_r4237519405)
- «Please use `AtomicPtr` instead of casting to/from usize» [tokio#8285](https://github.com/tokio-rs/tokio/pull/8285#discussion_r4079894185)
- «Let's define this as `libc::mode_t` so it always matches the platform» [tokio#8381](https://github.com/tokio-rs/tokio/pull/8381#discussion_r4281995870)
- «We always use strict equality unless there's a specific reason for weak equality.» [react#36971](https://github.com/react/react/pull/36971#discussion_r3590103280)
- «Is it worth adding a `default` case so we catch any future additions or erroneous data?» [prometheus#18051](https://github.com/prometheus/prometheus/pull/18051#discussion_r3712189887)

Корректность, ошибки, конкурентность:
- «nit: apparently `BucketFraction` sorts `tc.buckets` in place, so it should be cloned first» [prometheus#19963](https://github.com/prometheus/prometheus/pull/19963#discussion_r4228607171)
- «nit: idexing of array not guarded for overflow» [prometheus#20026](https://github.com/prometheus/prometheus/pull/20026#discussion_r4282236144)
- «`err2` is nil at this point, so we need to return some non-nil error here» [kubernetes#142143](https://github.com/kubernetes/kubernetes/pull/142143#discussion_r4324564083)
- «If toJSON() returns this then this will result in stack overflow» [react#37147](https://github.com/react/react/pull/37147#discussion_r3815516982)
- «This is a pre-existing issue, but this should actually close the fd.» [tokio#8603](https://github.com/tokio-rs/tokio/pull/8603#discussion_r4237605684)

Производительность (с оценкой масштаба):
- «A large `npm install` … can send tens of thousands of `Created` events, so we'd make tens of thousands of disk calls» [TypeScript#64909](https://github.com/microsoft/TypeScript/pull/64909#discussion_r4342906053)
- «This'd be faster as a `strings.Builder` …, but, it's probably not called enough to matter» [TypeScript#64372](https://github.com/microsoft/TypeScript/pull/64372#discussion_r4076370572)

## Eval-сценарии nits (15)

Изменение → senior → польза → шум (должен давать 0 кандидатов или REJECTED).

**Rust**
1. Новый builder `fn set_x(mut self, …) -> Self` без `#[must_use]`. → «добавить `#[must_use]`: `b.set_x(t);` молча теряет результат». Шум: «добавь doc-комментарии к struct».
2. `AtomicUsize` хранит указатель через `as usize`. → «используй `AtomicPtr<T>`». Шум: «unsafe опасен, перепроверь».
3. Новый helper + unit-тест ради `i32::try_from(x).unwrap_or(i32::MAX)` с одним вызовом. → «инлайнить, helper и тест не нужны». Шум: «добавь тест на отрицательные значения helper».
4. Комментарий у фикса: «раньше здесь был баг X, поэтому…». → «опиши инвариант, читатель не знает про баг». Шум: «добавь больше комментариев».
5. В цикле повторно читается значение, уже вычисленное до цикла. → «зачем перечитывать, есть `v` выше?» Шум: «вынеси цикл в функцию».

**TS/React**
6. `if (a == null || b == c)` в коде, где везде `===`. → «используем strict equality, если нет причины» (для `== null` — ок, не трогать). Шум: «включи strict mode в tsconfig».
7. Тест подменяет `console.error = jest.fn()` и не восстанавливает. → «`jest.spyOn`, иначе утечка в другие тесты; ошибки в тестах не глушим». Шум: «добавь ещё тестов».
8. `useEffect` выставляет state, производный от props. → «вычисли при рендере/в обработчике: эффект после paint даёт кадр с устаревшим UI». Шум: «оберни в `useMemo` для производительности» без измерения.
9. `index + 1` без пояснения в расчёте offset. → «откуда +1? комментарий». Шум: «вынеси 1 в константу `ONE`».
10. В PR про парсер — правка форматирования в несвязанном модуле. → «Looks unrelated — отдельным PR?» Шум: замечания к содержимому несвязанного кода.

**Go**
11. `stats := …` внутри функции файла, импортирующего `stats`. → «не затеняй импорт `stats`» + suggestion с другим именем. Шум: «переименуй в более описательное».
12. `var out []T; for … { out = append(out, …) }` при известном `len(in)`. → «`make([]T, 0, len(in))`». Шум: preallocate/`sync.Pool` на холодном пути с неизвестным размером.
13. Функция сортирует аргумент на месте; тест передаёт срез из таблицы кейсов. → «nit: функция сортирует на месте, клонируй». Шум: «используй `sort.Slice` вместо `slices.Sort`».
14. Поведение изменено, а комментарий выше говорит «this is constant». → «комментарий устарел, поправь». Шум: «добавь godoc ко всем функциям».
15. `isPrintable(name)` фактически проверяет «нужны ли кавычки». → «имя вводит в заблуждение; `nameRequiresQuoting` с инверсией». Шум: «`isPrintable` → `checkPrintable`».

Негативный контроль: `else` после `return` и формат при `gofmt`/`revive`/clippy в CI — дубль линтера.

## Рекомендации для discovery

1. Nit/improvement обязан иметь: anchor, механизм пользы (какую будущую ошибку/непонимание он предотвращает) и
   конкретную правку; без всех трёх — не кандидат. Предпочитать текст `suggestion`.
2. Сильнейший nit — расхождение с уже существующим в репозитории: хелпер, паттерн, соседний тест, конвенция. Требовать
   anchor на образец (`search_source`); «так принято вообще» — `PREFERENCE`.
3. Не предлагать то, что ловят rustfmt, clippy, gofmt, go vet, eslint, prettier, tsc; формат, порядок импортов,
   пробелы — запрещённые темы, если нет accepted rule.
4. Высокая ценность при малом шуме: устаревший/ложный комментарий, why-комментарий к магическому числу или deprecated
   API, затенение имени, мутация аргумента, `#[must_use]`/`default`, тест, который не может упасть или проходит
   без фикса, тест внутренностей вместо public API, несвязанный hunk в PR.
5. Неуверенность — вопрос с конкретным сценарием («что будет при пустом X?»), а не утверждение; такой кандидат
   остаётся `UNRESOLVED`, а не nit. Вопросы «для себя» и похвала не выдаются (Bosu: not useful).
6. Performance-nit — только с аргументом о масштабе (горячий путь, размер входа); иначе не выдавать.
7. Предпочитать удаление и инлайн добавлению абстракции; предлагать новый helper только при ≥3 повторах в diff.
8. Проблемы в нетронутом коде — максимум одна пометка «pre-existing», не nit к изменению.
9. Лимит: не больше ~3 nits на review unit, и меньше на больших PR (полезность падает с числом файлов);
   отбирать по пункту 4. Ноль nits — нормальный ответ (уже в prompt).
10. Формулировка в выводе: «nit:» + место + причина + правка, 1–2 фразы, идентификаторы в backticks.
