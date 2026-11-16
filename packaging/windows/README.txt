CrossFX for Windows
===================

CrossFX is the FxSound audio enhancer, ported to Rust: the equalizer, the effects and the
factory presets of the original FxSound for everything Windows plays. It is made by the CrossFX
contributors and is not FxSound LLC's FxSound; the two can be installed side by side.

It runs on 64-bit Windows 10 version 1703 (build 15063) or later and on Windows 11.

CrossFX needs a virtual playback device, which it does not install: VB-Audio's VB-CABLE
(https://vb-audio.com/Cable/, donationware that you install yourself) or another virtual cable,
or the FxSound Audio Enhancer driver that comes with the original FxSound. While the original
FxSound runs, CrossFX leaves the speakers to it: it neither processes their sound nor changes the
default playback device.

What is in this folder

  crossfx.exe       the program
  presets\          the factory presets: Factsoft and BonusPresets (.fac), Input (.toml)
  LICENSE           CrossFX's licence, the GNU Affero General Public License 3 or later
  COPYING.rnnoise   BSD-3-Clause, for the RNNoise noise suppression built into crossfx.exe
  OFL.noto          SIL Open Font License 1.1, for the Noto fonts built into crossfx.exe
  OFL.manrope       SIL Open Font License 1.1, for the Manrope font built into crossfx.exe
  UFL.ubuntu        Ubuntu Font Licence 1.0, for the Ubuntu Light font built into crossfx.exe
  MIT.hack          MIT and Bitstream Vera licences, for the Hack font built into crossfx.exe
  MIT.emoji-icons   MIT licence, for the icons (from emoji-icon-font) built into crossfx.exe
  THIRD-PARTY-NOTICES.txt
                    licences and copyright notices of the Rust libraries built into crossfx.exe
  README.txt        this file

The setup program and the .msi install CrossFX into Program Files, and only there, for every
account on the computer; it can be uninstalled from the list of installed apps in Windows Settings.
The .zip needs no installation: unpack it into a folder of your own, such as one in your user
folder (%LOCALAPPDATA%\Programs), and run crossfx.exe, keeping the presets folder beside it. Not
into a folder other accounts can change, as they can any new folder directly on C:\: whoever
replaces crossfx.exe there decides what runs when you start it. Before deleting a copy from the
.zip, turn off "Launch on system startup" in its settings, or Windows keeps trying to start it at
every login.

Source code, issues and releases: https://github.com/blackixxce12/CrossFX


CrossFX для Windows
===================

CrossFX — это программа для улучшения звука FxSound, перенесённая на Rust: эквалайзер, эффекты
и заводские пресеты оригинального FxSound для всего, что воспроизводит Windows. Её делают
участники проекта CrossFX, и это не FxSound от FxSound LLC; обе программы можно установить на
один компьютер.

Для работы CrossFX нужно виртуальное устройство воспроизведения, которое программа не
устанавливает сама: VB-CABLE от VB-Audio (https://vb-audio.com/Cable/; распространяется за
добровольные пожертвования, устанавливаете его вы сами), другой виртуальный кабель или драйвер
FxSound Audio Enhancer из оригинального FxSound. Пока работает оригинальный FxSound, CrossFX
уступает ему динамики: не обрабатывает их звук и не меняет устройство воспроизведения
по умолчанию.

Содержимое этой папки

  crossfx.exe       сама программа
  presets\          заводские пресеты: Factsoft и BonusPresets (.fac), Input (.toml)
  LICENSE           лицензия CrossFX — GNU Affero General Public License версии 3 или более
                    поздней
  COPYING.rnnoise   лицензия BSD-3-Clause для встроенного в crossfx.exe шумоподавления RNNoise
  OFL.noto          SIL Open Font License 1.1 для встроенных в crossfx.exe шрифтов Noto
  OFL.manrope       SIL Open Font License 1.1 для встроенного в crossfx.exe шрифта Manrope
  UFL.ubuntu        Ubuntu Font Licence 1.0 для встроенного в crossfx.exe шрифта Ubuntu Light
  MIT.hack          лицензии MIT и Bitstream Vera для встроенного в crossfx.exe шрифта Hack
  MIT.emoji-icons   лицензия MIT для встроенных в crossfx.exe значков (из emoji-icon-font)
  THIRD-PARTY-NOTICES.txt
                    лицензии и уведомления об авторских правах библиотек Rust, встроенных в
                    crossfx.exe
  README.txt        этот файл

Программа установки и пакет .msi устанавливают CrossFX в Program Files, и только туда, для
всех учётных записей компьютера; удалить его можно из списка установленных приложений в
параметрах Windows. Архив .zip устанавливать не нужно: распакуйте его в свою папку, например в
папку внутри вашего профиля (%LOCALAPPDATA%\Programs), и запустите crossfx.exe, не отделяя от
него папку presets. Только не в папку, которую могут изменять другие учётные записи, как любую
новую папку прямо на диске C:\: кто заменит там crossfx.exe, тот и решает, что запустится у вас.
Прежде чем удалить копию из архива, выключите в её настройках параметр «Автоматически запускать
при входе в систему», иначе Windows будет пытаться запускать её при каждом входе.

Исходный код, сообщения об ошибках и выпуски: https://github.com/blackixxce12/CrossFX
