execute if entity @s[tag=arnis.w_red1] if score #tl arnis matches 0..159 run return run function arnis:walk/go
execute if entity @s[tag=arnis.w_red2] if score #tl arnis matches 340..499 run return run function arnis:walk/go
execute if entity @s[tag=arnis.w_kerb] store result score #gap arnis.walk run function arnis:walk/gap
execute if entity @s[tag=arnis.w_kerb] if score #gap arnis.walk matches 1 run return run function arnis:walk/go
scoreboard players remove @s arnis.ww 1
execute if score @s arnis.ww matches ..0 run function arnis:walk/go
