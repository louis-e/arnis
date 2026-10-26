scoreboard players set #lo arnis.ds 0
scoreboard players set #hi arnis.ds 22
function arnis:disaster/util/scatter
execute at @s positioned over motion_blocking run function arnis:disaster/snow/settle
kill @s
