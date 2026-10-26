execute if score #budget arnis.ds matches ..0 run return 0
scoreboard players operation #cx arnis.ds = #px arnis.ds
scoreboard players operation #cx arnis.ds += #dx arnis.ds
scoreboard players operation #cz arnis.ds = #pz arnis.ds
scoreboard players operation #cz arnis.ds += #dz arnis.ds
execute store result storage arnis:ds scan.x int 1 run scoreboard players get #cx arnis.ds
execute store result storage arnis:ds scan.z int 1 run scoreboard players get #cz arnis.ds
scoreboard players operation #cx arnis.ds *= #64 arnis
scoreboard players operation #cz arnis.ds *= #64 arnis
execute store result storage arnis:ds scan.x0 int 1 run scoreboard players get #cx arnis.ds
execute store result storage arnis:ds scan.z0 int 1 run scoreboard players get #cz arnis.ds
scoreboard players add #cx arnis.ds 63
scoreboard players add #cz arnis.ds 63
execute store result storage arnis:ds scan.x1 int 1 run scoreboard players get #cx arnis.ds
execute store result storage arnis:ds scan.z1 int 1 run scoreboard players get #cz arnis.ds
execute if score #engine arnis.ds matches 1 run return run function arnis:disaster/snow/need with storage arnis:ds scan
function arnis:disaster/water/need with storage arnis:ds scan
