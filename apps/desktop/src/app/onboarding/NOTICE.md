# welcome-sound.m4a

The first launch's welcome music is rendered by `scripts/render-welcome-sound.mjs` from an
original score, played on the instruments of the **FluidR3 Mono GM** SoundFont (version 2.312),
as MuseScore 2.1 ships it:
<https://github.com/musescore/MuseScore/raw/2.1/share/sound/FluidR3Mono_GM.sf3>
(its licence: <https://github.com/musescore/MuseScore/blob/2.1/share/sound/FluidR3Mono_License.md>).

FluidR3 Mono GM is released under the MIT licence:

- Original stereo version by Frank Wen, Copyright © 2000-2002, 2008
- Mono version by Michael Cowgill, Copyright © 2014-16
- Temple Blocks instrument provided by Ethan Winer, Copyright © 2002
- Drumline Percussion provided by Michael Schorsch, Copyright © 2016

> Permission is hereby granted, free of charge, to any person obtaining a copy of this software
> and associated documentation files (the "Software"), to deal in the Software without
> restriction, including without limitation the rights to use, copy, modify, merge, publish,
> distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the
> Software is furnished to do so, subject to the following conditions:
>
> The above copyright notice and this permission notice shall be included in all copies or
> substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
> BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
> NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
> DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
> OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

The SoundFont itself isn't in the repo: the script downloads it, checks its SHA-256 and renders
with spessasynth_core (Apache-2.0), a build-time dependency only. Only the rendered sound ships.
