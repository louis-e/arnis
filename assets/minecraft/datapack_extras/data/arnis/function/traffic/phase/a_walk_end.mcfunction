execute as @e[type=block_display,tag=arnis.tl_pa_g] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_pa_r] run function arnis:traffic/on
scoreboard players set #tl_pa arnis 0
function arnis:traffic/switched
