execute as @e[type=item_display,tag=arnis.tl_a] run function arnis:traffic/car/green
execute as @e[type=item_display,tag=arnis.tl_pb] run function arnis:traffic/walk/green
scoreboard players set #tl_a arnis 0
scoreboard players set #tl_pb arnis 1
function arnis:traffic/switched
