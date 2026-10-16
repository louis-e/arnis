# A night lasts 500 s; one that ended well before that was slept or commanded through.
execute if score #night_t arnis.ob matches ..449 run return run function arnis:outbreak/no_sleep
function arnis:outbreak/dawn
