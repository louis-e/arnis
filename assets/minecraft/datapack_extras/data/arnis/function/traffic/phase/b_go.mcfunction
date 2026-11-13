execute as @e[type=item_display,tag=arnis.tl_b] run function arnis:traffic/car/green
execute as @e[type=item_display,tag=arnis.tl_pa] run function arnis:traffic/walk/green
scoreboard players set #tl_b arnis 0
scoreboard players set #tl_pa arnis 1
function arnis:traffic/switched
