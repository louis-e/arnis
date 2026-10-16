execute as @e[type=block_display,tag=arnis.tl_pb_g] run function arnis:traffic/off
execute as @e[type=block_display,tag=arnis.tl_pb_r] run function arnis:traffic/on
scoreboard players set #tl_pb arnis 0
function arnis:traffic/switched
