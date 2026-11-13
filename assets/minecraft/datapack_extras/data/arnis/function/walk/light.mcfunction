# Walkers start over only early in their green, so they are across before the cars go.
execute if data storage arnis:walk p{g:1b} unless score #tl arnis matches 0..159 run tag @s add arnis.w_red1
execute if data storage arnis:walk p{g:2b} unless score #tl arnis matches 340..499 run tag @s add arnis.w_red2
execute if entity @s[tag=arnis.w_red1] run scoreboard players set @s arnis.ww 700
execute if entity @s[tag=arnis.w_red2] run scoreboard players set @s arnis.ww 700
